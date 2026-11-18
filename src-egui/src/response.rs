use egui::{Color32, RichText};
use httpdeck_lib::models::RequestResult;

const BODY_PREVIEW_LIMIT: usize = 200_000;

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
}

pub fn ui(ui: &mut egui::Ui, result: Option<&RequestResult>, error: Option<&str>) {
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

    let Some(result) = result else {
        ui.weak("尚未执行请求");
        return;
    };

    let tab_id = egui::Id::new("response_tab");
    let view_id = egui::Id::new("response_body_view");
    let mut tab = ui
        .memory(|memory| memory.data.get_temp::<ResponseTab>(tab_id))
        .unwrap_or(ResponseTab::Body);
    let mut body_view = ui
        .memory(|memory| memory.data.get_temp::<BodyView>(view_id))
        .unwrap_or(BodyView::Pretty);

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
            for (view, label) in [(BodyView::Pretty, "美化"), (BodyView::Raw, "原始")] {
                if ui.selectable_label(body_view == view, label).clicked() {
                    body_view = view;
                }
            }
        }
    });
    ui.separator();

    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if let Some(error) = &result.error {
                ui.colored_label(Color32::from_rgb(230, 100, 100), error);
            }
            if let Some(error) = &result.script_error {
                ui.colored_label(
                    Color32::from_rgb(230, 170, 70),
                    format!("脚本错误：{error}"),
                );
            }

            match tab {
                ResponseTab::Body => {
                    if result.body_is_binary {
                        ui.weak("二进制响应，以下内容为 Base64 编码");
                    }
                    let text = body_text(result, body_view == BodyView::Pretty);
                    selectable_code(ui, text);
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
            }
        });

    ui.memory_mut(|memory| {
        memory.data.insert_temp(tab_id, tab);
        memory.data.insert_temp(view_id, body_view);
    });
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

fn body_text(result: &RequestResult, pretty: bool) -> String {
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
            let mut output = ctx.run_ui(input, |ui| super::ui(ui, Some(&result), None));
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
}
