use egui::{Color32, RichText};
use httpdeck_lib::models::RequestResult;

const BODY_PREVIEW_LIMIT: usize = 200_000;
/// 超过该大小关闭美化，并按行虚拟滚动
const VIRTUAL_BODY_LIMIT: usize = 1_000_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResponseTab {
    Body,
    Headers,
    Tests,
    Console,
    Request,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BodyView {
    Pretty,
    Raw,
    Tree,
}

/// 渲染响应面板；返回用户点击切换的结果索引
pub fn ui(
    ui: &mut egui::Ui,
    result: Option<&RequestResult>,
    error: Option<&str>,
    results: &[(usize, &RequestResult)],
    selected: Option<usize>,
) -> Option<usize> {
    let mut clicked_result = None;
    ui.horizontal(|ui| {
        ui.strong("响应");
        if let Some(result) = result {
            let color = if result.error.is_some() {
                Color32::from_rgb(230, 100, 100)
            } else if result.status >= 400 {
                Color32::from_rgb(230, 170, 70)
            } else {
                Color32::from_rgb(90, 190, 120)
            };
            ui.colored_label(color, format!("{} {}", result.status, result.status_text));
            ui.label(format!("{} ms", result.duration_ms));
            ui.label(format_size(result.size));
            if let Some(mime) = &result.mime_type {
                ui.weak(mime);
            }
        }
        if let Some(error) = error {
            ui.colored_label(Color32::from_rgb(230, 100, 100), error);
        }
    });

    ui.separator();

    // 多结果切换条：整个文件运行后可在不同请求的结果之间切换
    if results.len() > 1 {
        ui.horizontal_wrapped(|ui| {
            ui.weak("全部结果");
            for (index, item) in results {
                let label = format!("#{} {} {}", index + 1, item.method, item.status);
                let response = ui
                    .selectable_label(selected == Some(*index), label)
                    .on_hover_text(format!("{} {}", item.method, item.name));
                if response.clicked() {
                    clicked_result = Some(*index);
                }
            }
        });
        ui.separator();
    }

    let Some(result) = result else {
        ui.weak("尚未执行请求");
        return clicked_result;
    };

    let tab_id = egui::Id::new("response_tab");
    let view_id = egui::Id::new("response_body_view");
    let mut tab = ui
        .memory(|memory| memory.data.get_temp::<ResponseTab>(tab_id))
        .unwrap_or(ResponseTab::Body);
    let mut body_view = ui
        .memory(|memory| memory.data.get_temp::<BodyView>(view_id))
        .unwrap_or(BodyView::Pretty);

    // 只在响应体不大且为合法 JSON 时提供树形视图，避免超大响应解析卡顿
    let body_json = (!result.body_is_binary && result.body.len() <= BODY_PREVIEW_LIMIT)
        .then(|| serde_json::from_str::<serde_json::Value>(&result.body).ok())
        .flatten();

    let search_id = egui::Id::new("response_search");
    let mut search = ui
        .memory(|memory| memory.data.get_temp::<String>(search_id))
        .unwrap_or_default();
    let mut copy_current = false;
    let mut copy_curl = false;

    ui.horizontal(|ui| {
        select_tab(ui, &mut tab, ResponseTab::Body, "响应体".to_owned());
        select_tab(
            ui,
            &mut tab,
            ResponseTab::Headers,
            format!("响应头 {}", result.response_headers.len()),
        );
        select_tab(
            ui,
            &mut tab,
            ResponseTab::Tests,
            format!("测试 {}", result.tests.len()),
        );
        select_tab(
            ui,
            &mut tab,
            ResponseTab::Console,
            format!("控制台 {}", result.logs.len()),
        );
        select_tab(ui, &mut tab, ResponseTab::Request, "实际请求".to_owned());
        if tab == ResponseTab::Body && !result.body_is_binary {
            ui.separator();
            let huge = result.body.len() > VIRTUAL_BODY_LIMIT;
            if huge && body_view != BodyView::Raw {
                body_view = BodyView::Raw;
            }
            if huge {
                ui.weak("超大响应：已关闭美化，按行虚拟滚动");
            } else {
                for (view, label) in [(BodyView::Pretty, "美化"), (BodyView::Raw, "原始")] {
                    if ui.selectable_label(body_view == view, label).clicked() {
                        body_view = view;
                    }
                }
                if body_json.is_some()
                    && ui
                        .selectable_label(body_view == BodyView::Tree, "树形")
                        .clicked()
                {
                    body_view = BodyView::Tree;
                }
            }
            if body_view != BodyView::Tree {
                ui.add(
                    egui::TextEdit::singleline(&mut search)
                        .id(search_id)
                        .hint_text("搜索")
                        .desired_width(120.0),
                );
            }
        }
        ui.separator();
        if ui
            .button("复制")
            .on_hover_text("复制当前标签内容")
            .clicked()
        {
            copy_current = true;
        }
        if ui
            .button("cURL")
            .on_hover_text("复制为 curl 命令")
            .clicked()
        {
            copy_curl = true;
        }
    });
    ui.separator();

    if let Some(error) = &result.error {
        ui.colored_label(Color32::from_rgb(230, 100, 100), error);
    }
    if let Some(error) = &result.script_error {
        ui.colored_label(
            Color32::from_rgb(230, 170, 70),
            format!("脚本错误：{error}"),
        );
    }

    // 超大响应直接按行虚拟化，避免一次性创建数万个文本控件
    let virtual_body = tab == ResponseTab::Body
        && body_view != BodyView::Tree
        && result.body.len() > VIRTUAL_BODY_LIMIT;
    if virtual_body {
        virtual_body_view(ui, result, &search);
    } else {
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| match tab {
                ResponseTab::Body => {
                    if result.body_is_binary {
                        ui.weak("二进制响应，以下内容为 Base64 编码");
                    }
                    if body_view == BodyView::Tree {
                        match &body_json {
                            Some(value) => json_tree(ui, value, "root", 0),
                            None => {
                                ui.weak("响应体不是有效 JSON，或超过 200 KB 未提供树形视图");
                            }
                        }
                    } else {
                        let text = body_text(result, body_view == BodyView::Pretty);
                        let (shown, matched) = filter_lines(&text, &search);
                        if !search.is_empty() {
                            ui.weak(format!("匹配 {matched} 行"));
                        }
                        selectable_code(ui, shown);
                    }
                }
                ResponseTab::Headers => {
                    if result.response_headers.is_empty() {
                        ui.weak("没有响应头");
                    }
                    for header in &result.response_headers {
                        ui.monospace(format!("{}: {}", header.name, header.value));
                    }
                }
                ResponseTab::Tests => {
                    if result.tests.is_empty() {
                        ui.weak("该请求没有测试断言");
                    }
                    for test in &result.tests {
                        let color = if test.passed {
                            Color32::from_rgb(90, 190, 120)
                        } else {
                            Color32::from_rgb(230, 100, 100)
                        };
                        let mut text = format!(
                            "{} {}",
                            if test.passed { "PASS" } else { "FAIL" },
                            test.name
                        );
                        if !test.passed && !test.message.is_empty() {
                            text.push_str(&format!(" - {}", test.message));
                        }
                        ui.colored_label(color, text);
                    }
                }
                ResponseTab::Console => {
                    if result.logs.is_empty() {
                        ui.weak("没有脚本日志（可使用 client.log 输出）");
                    }
                    for (index, line) in result.logs.iter().enumerate() {
                        selectable_code(ui, format!("{}  {}", index + 1, line));
                    }
                }
                ResponseTab::Request => selectable_code(ui, actual_request_text(result)),
            });
    }

    if copy_current {
        let text = match tab {
            ResponseTab::Body => body_text(result, body_view == BodyView::Pretty),
            ResponseTab::Headers => result
                .response_headers
                .iter()
                .map(|header| format!("{}: {}", header.name, header.value))
                .collect::<Vec<_>>()
                .join("\n"),
            ResponseTab::Tests => result
                .tests
                .iter()
                .map(|test| {
                    format!(
                        "{} {}",
                        if test.passed { "PASS" } else { "FAIL" },
                        test.name
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
            ResponseTab::Console => result.logs.join("\n"),
            ResponseTab::Request => actual_request_text(result),
        };
        ui.ctx().copy_text(text);
    }
    if copy_curl {
        ui.ctx().copy_text(curl_text(result));
    }

    ui.memory_mut(|memory| {
        memory.data.insert_temp(tab_id, tab);
        memory.data.insert_temp(view_id, body_view);
        memory.data.insert_temp(search_id, search);
    });
    clicked_result
}

fn select_tab(ui: &mut egui::Ui, current: &mut ResponseTab, tab: ResponseTab, label: String) {
    if ui.selectable_label(*current == tab, label).clicked() {
        *current = tab;
    }
}

fn selectable_code(ui: &mut egui::Ui, text: String) {
    ui.add(
        egui::Label::new(RichText::new(text).monospace())
            .selectable(true)
            .wrap_mode(egui::TextWrapMode::Extend),
    );
}

/// 超大响应：只渲染可见行，保持滚动流畅
fn virtual_body_view(ui: &mut egui::Ui, result: &RequestResult, search: &str) {
    let keyword = search.trim().to_lowercase();
    let lines: Vec<&str> = if keyword.is_empty() {
        result.body.lines().collect()
    } else {
        result
            .body
            .lines()
            .filter(|line| line.to_lowercase().contains(&keyword))
            .collect()
    };
    if !keyword.is_empty() {
        ui.weak(format!("匹配 {} 行", lines.len()));
    }
    ui.weak(format!("共 {} 行，按行虚拟滚动", lines.len()));
    let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
    egui::ScrollArea::vertical()
        .id_salt("virtual_body")
        .auto_shrink([false, false])
        .show_rows(ui, row_height, lines.len(), |ui, range| {
            for line in &lines[range] {
                ui.add(
                    egui::Label::new(RichText::new(*line).monospace())
                        .selectable(true)
                        .wrap_mode(egui::TextWrapMode::Extend),
                );
            }
        });
}

fn body_text(result: &RequestResult, pretty: bool) -> String {
    // 超大响应不做美化，复制时保留完整内容
    if result.body.len() > VIRTUAL_BODY_LIMIT {
        return result.body.clone();
    }
    let mut text = if pretty {
        pretty_body(result)
    } else {
        result.body.clone()
    };
    if text.len() > BODY_PREVIEW_LIMIT {
        let mut boundary = BODY_PREVIEW_LIMIT;
        while !text.is_char_boundary(boundary) {
            boundary -= 1;
        }
        text.truncate(boundary);
        text.push_str("\n\n…（响应体过大，仅显示前 200 KB）");
    }
    text
}

fn pretty_body(result: &RequestResult) -> String {
    if result.body_is_binary {
        return result.body.clone();
    }
    let mime = result.mime_type.as_deref().unwrap_or("").to_lowercase();
    let trimmed = result.body.trim();
    if !mime.contains("json") && !trimmed.starts_with(['{', '[']) {
        return result.body.clone();
    }
    serde_json::from_str::<serde_json::Value>(&result.body)
        .and_then(|value| serde_json::to_string_pretty(&value))
        .unwrap_or_else(|_| result.body.clone())
}

const JSON_MAX_DEPTH: usize = 32;

/// 递归渲染 JSON 树；对象 / 数组节点可折叠
fn json_tree(ui: &mut egui::Ui, value: &serde_json::Value, path: &str, depth: usize) {
    if depth > JSON_MAX_DEPTH {
        ui.weak("…（嵌套过深）");
        return;
    }
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                let id = format!("{path}/{key}");
                let label = format!("{key}  {}", json_summary(child));
                if json_is_container(child) {
                    egui::CollapsingHeader::new(label)
                        .id_salt(&id)
                        .default_open(depth < 2)
                        .show(ui, |ui| json_tree(ui, child, &id, depth + 1));
                } else {
                    ui.label(label);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                let id = format!("{path}[{index}]");
                let label = format!("[{index}]  {}", json_summary(child));
                if json_is_container(child) {
                    egui::CollapsingHeader::new(label)
                        .id_salt(&id)
                        .default_open(depth < 2)
                        .show(ui, |ui| json_tree(ui, child, &id, depth + 1));
                } else {
                    ui.label(label);
                }
            }
        }
        other => {
            ui.label(json_summary(other));
        }
    }
}

fn json_is_container(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => !map.is_empty(),
        serde_json::Value::Array(items) => !items.is_empty(),
        _ => false,
    }
}

/// 节点摘要：容器显示条目数，字符串截断显示
fn json_summary(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            if map.is_empty() {
                "{}".to_owned()
            } else {
                format!("{{…}} {} 项", map.len())
            }
        }
        serde_json::Value::Array(items) => {
            if items.is_empty() {
                "[]".to_owned()
            } else {
                format!("[…] {} 项", items.len())
            }
        }
        serde_json::Value::String(text) => {
            if text.chars().count() > 80 {
                let mut short: String = text.chars().take(80).collect();
                short.push('…');
                format!("\"{short}\"")
            } else {
                format!("\"{text}\"")
            }
        }
        other => other.to_string(),
    }
}

/// 按关键词过滤文本，返回（过滤后文本，匹配行数）
fn filter_lines(text: &str, query: &str) -> (String, usize) {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return (text.to_owned(), 0);
    }
    let mut kept = Vec::new();
    for line in text.lines() {
        if line.to_lowercase().contains(&query) {
            kept.push(line);
        }
    }
    (kept.join("\n"), kept.len())
}

/// 把实际请求转换为可直接执行的 curl 命令
fn curl_text(result: &RequestResult) -> String {
    let escape = |value: &str| value.replace('\'', "'\\''");
    let mut parts = vec![format!(
        "curl -X {} '{}'",
        result.method,
        escape(&result.url)
    )];
    for header in &result.request_headers {
        parts.push(format!(
            "-H '{}: {}'",
            escape(&header.name),
            escape(&header.value)
        ));
    }
    if let Some(body) = &result.request_body {
        parts.push(format!("--data-raw '{}'", escape(body)));
    }
    parts.join(" \\\n  ")
}

fn actual_request_text(result: &RequestResult) -> String {
    let mut lines = vec![format!("{} {}", result.method, result.url)];
    lines.extend(
        result
            .request_headers
            .iter()
            .map(|header| format!("{}: {}", header.name, header.value)),
    );
    if let Some(body) = &result.request_body {
        lines.push(String::new());
        lines.push(body.clone());
    }
    lines.join("\n")
}

fn format_size(size: usize) -> String {
    if size >= 1024 * 1024 {
        format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
    } else if size >= 1024 {
        format!("{:.1} KB", size as f64 / 1024.0)
    } else {
        format!("{size} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpdeck_lib::models::{KeyValue, TestResult};

    fn sample_result() -> RequestResult {
        RequestResult {
            request_index: 0,
            name: "JSON 示例".to_owned(),
            method: "POST".to_owned(),
            url: "http://127.0.0.1:8080/api?final=true".to_owned(),
            request_headers: vec![
                KeyValue {
                    name: "Content-Type".to_owned(),
                    value: "application/json".to_owned(),
                },
                KeyValue {
                    name: "X-Debug".to_owned(),
                    value: "egui".to_owned(),
                },
            ],
            request_body: Some("{\"input\":42}".to_owned()),
            status: 200,
            status_text: "OK".to_owned(),
            response_headers: vec![KeyValue {
                name: "Content-Type".to_owned(),
                value: "application/json".to_owned(),
            }],
            body: "{\"name\":\"HttpDeck\",\"items\":[1,2]}".to_owned(),
            body_is_binary: false,
            mime_type: Some("application/json; charset=utf-8".to_owned()),
            size: 38,
            duration_ms: 12,
            tests: vec![TestResult {
                name: "状态为 200".to_owned(),
                passed: true,
                message: String::new(),
            }],
            logs: vec!["response: 200".to_owned()],
            script_error: None,
            saved_to: None,
            error: None,
            timeout_ms: None,
            no_redirect: false,
            redirected: false,
        }
    }

    #[test]
    fn json_body_can_be_pretty_or_raw() {
        let result = sample_result();
        assert_eq!(body_text(&result, false), result.body);
        assert_eq!(
            pretty_body(&result),
            "{\n  \"items\": [\n    1,\n    2\n  ],\n  \"name\": \"HttpDeck\"\n}"
        );
    }

    #[test]
    fn invalid_or_non_json_body_is_unchanged() {
        let mut result = sample_result();
        result.body = "{invalid".to_owned();
        assert_eq!(pretty_body(&result), "{invalid");
        result.body = "plain text".to_owned();
        result.mime_type = Some("text/plain".to_owned());
        assert_eq!(pretty_body(&result), "plain text");
        result.body = "{\"name\":\"HttpDeck\"}".to_owned();
        result.body_is_binary = true;
        assert_eq!(pretty_body(&result), result.body);
    }

    #[test]
    fn actual_request_contains_final_url_headers_and_body() {
        assert_eq!(
            actual_request_text(&sample_result()),
            "POST http://127.0.0.1:8080/api?final=true\nContent-Type: application/json\nX-Debug: egui\n\n{\"input\":42}"
        );
    }

    #[test]
    fn body_preview_truncates_on_utf8_boundary() {
        let mut result = sample_result();
        result.body = "界".repeat(BODY_PREVIEW_LIMIT);
        result.mime_type = Some("text/plain".to_owned());
        let text = body_text(&result, true);
        assert!(text.ends_with("…（响应体过大，仅显示前 200 KB）"));
        assert!(text.len() < result.body.len());
    }

    #[test]
    fn filter_keeps_only_matching_lines() {
        let text = "{\n  \"name\": \"HttpDeck\",\n  \"items\": [1,2]\n}";
        assert_eq!(filter_lines(text, "").1, 0);
        let (shown, matched) = filter_lines(text, "name");
        assert_eq!(matched, 1);
        assert_eq!(shown, "  \"name\": \"HttpDeck\",");
        assert_eq!(filter_lines(text, "NOTHING").1, 0);
    }

    #[test]
    fn curl_text_contains_method_headers_and_body() {
        let text = curl_text(&sample_result());
        assert!(text.starts_with("curl -X POST 'http://127.0.0.1:8080/api?final=true'"));
        assert!(text.contains("-H 'Content-Type: application/json'"));
        assert!(text.contains("--data-raw '{\"input\":42}'"));
    }

    #[test]
    fn multi_results_render_switch_chips() {
        let ctx = egui::Context::default();
        let first = sample_result();
        let mut second = sample_result();
        second.request_index = 1;
        second.method = "GET".to_owned();
        second.status = 404;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 600.0),
                )),
                ..Default::default()
            },
            |ui| {
                super::ui(
                    ui,
                    Some(&first),
                    None,
                    &[(0, &first), (1, &second)],
                    Some(0),
                );
            },
        );
        output.textures_delta.clear();
        let texts: Vec<String> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        assert!(
            texts.iter().any(|text| text.contains("#2 GET 404")),
            "应渲染第二个结果的可切换标签，实际: {texts:?}"
        );
    }

    /// JSON 响应可切换树形视图，节点按折叠头渲染
    #[test]
    fn json_tree_renders_nodes() {
        let ctx = egui::Context::default();
        ctx.memory_mut(|memory| {
            memory
                .data
                .insert_temp(egui::Id::new("response_body_view"), BodyView::Tree);
        });
        let result = sample_result();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 600.0),
                )),
                ..Default::default()
            },
            |ui| {
                super::ui(ui, Some(&result), None, &[], None);
            },
        );
        output.textures_delta.clear();
        let texts: Vec<String> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        assert!(
            texts.iter().any(|text| text == "name  \"HttpDeck\""),
            "应渲染字符串节点: {texts:?}"
        );
        assert!(
            texts.iter().any(|text| text == "items  […] 2 项"),
            "数组节点应显示条目数: {texts:?}"
        );
    }

    /// 非 JSON 响应不显示树形按钮
    #[test]
    fn json_tree_button_only_for_json() {
        let draw = |result: &RequestResult| {
            let ctx = egui::Context::default();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 600.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    super::ui(ui, Some(result), None, &[], None);
                },
            );
            output.textures_delta.clear();
            output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect::<Vec<String>>()
        };
        let texts = draw(&sample_result());
        assert!(texts.iter().any(|text| text == "树形"), "{texts:?}");

        let mut plain = sample_result();
        plain.body = "plain text".to_owned();
        plain.mime_type = Some("text/plain".to_owned());
        let texts = draw(&plain);
        assert!(!texts.iter().any(|text| text == "树形"), "{texts:?}");
    }

    #[test]
    fn raw_body_button_is_clickable() {
        let ctx = egui::Context::default();
        let result = sample_result();
        let draw = |events| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 600.0),
                )),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                super::ui(ui, Some(&result), None, &[], None);
            });
            output.textures_delta.clear();
            output
        };
        let output = draw(Vec::new());
        let raw_pos = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == "原始" => {
                    Some(text.pos + text.galley.size() / 2.0)
                }
                _ => None,
            })
            .expect("应渲染原始响应按钮");
        draw(vec![
            egui::Event::PointerMoved(raw_pos),
            egui::Event::PointerButton {
                pos: raw_pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        draw(vec![egui::Event::PointerButton {
            pos: raw_pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        draw(Vec::new());
        let view = ctx.memory(|memory| {
            memory
                .data
                .get_temp::<BodyView>(egui::Id::new("response_body_view"))
        });
        assert!(view == Some(BodyView::Raw), "点击后应切换到原始视图");
    }

    /// 超过 1 MB 的响应应关闭美化并只渲染可见行
    #[test]
    fn huge_body_virtualizes_and_disables_pretty() {
        let mut result = sample_result();
        result.body = (0..120_000)
            .map(|index| format!("line {index}\n"))
            .collect::<String>();
        result.mime_type = Some("application/json".to_owned());
        assert!(result.body.len() > VIRTUAL_BODY_LIMIT);

        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 600.0),
                )),
                ..Default::default()
            },
            |ui| {
                super::ui(ui, Some(&result), None, &[], None);
            },
        );
        output.textures_delta.clear();
        let texts: Vec<String> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        assert!(
            texts
                .iter()
                .any(|text| text == "超大响应：已关闭美化，按行虚拟滚动"),
            "{texts:?}"
        );
        assert!(!texts.iter().any(|text| text == "美化"), "{texts:?}");
        assert!(texts.iter().any(|text| text == "line 0"), "{texts:?}");
        assert!(
            !texts.iter().any(|text| text == "line 119999"),
            "末尾行不应被渲染（虚拟滚动）: {texts:?}"
        );
    }

    /// 超大响应复制时不应被截断
    #[test]
    fn huge_body_copy_is_not_truncated() {
        let mut result = sample_result();
        result.body = "x".repeat(VIRTUAL_BODY_LIMIT + 10);
        let text = super::body_text(&result, true);
        assert_eq!(text.len(), result.body.len());
        assert!(!text.contains("仅显示前 200 KB"));
    }
}
