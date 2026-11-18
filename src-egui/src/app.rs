use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use httpdeck_lib::models::{FileNode, RequestResult};
use httpdeck_lib::parser::{self, ParsedHttpFile};
use httpdeck_lib::{recent, workspace};

use crate::{response, tree};

enum TaskMsg {
    Done(Result<RequestResult, String>),
    FileDone(Result<Vec<RequestResult>, String>),
}

pub struct HttpDeckApp {
    state: Arc<httpdeck_lib::AppState>,
    runtime: tokio::runtime::Runtime,
    tx: Sender<TaskMsg>,
    rx: Receiver<TaskMsg>,

    root: Option<PathBuf>,
    tree: Option<FileNode>,
    envs: Vec<String>,
    selected_env: Option<String>,

    open_file: Option<PathBuf>,
    editor_text: String,
    dirty: bool,
    parsed: Option<ParsedHttpFile>,
    selected_request: Option<usize>,
    results: HashMap<usize, RequestResult>,

    running: usize,
    status: String,
    error: Option<String>,
}

impl HttpDeckApp {
    pub fn new() -> Self {
        let (tx, rx) = channel();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("创建异步运行时失败");
        let mut app = Self {
            state: Arc::new(httpdeck_lib::AppState::default()),
            runtime,
            tx,
            rx,
            root: None,
            tree: None,
            envs: Vec::new(),
            selected_env: None,
            open_file: None,
            editor_text: String::new(),
            dirty: false,
            parsed: None,
            selected_request: None,
            results: HashMap::new(),
            running: 0,
            status: String::new(),
            error: None,
        };
        if let Some(root) = recent::list()
            .into_iter()
            .map(PathBuf::from)
            .find(|path| path.is_dir())
        {
            app.load_workspace(root);
        }
        app
    }

    fn load_workspace(&mut self, dir: PathBuf) {
        recent::record(&dir);
        let config = workspace::load_config(&dir);
        let envs = read_env_names(&dir);
        let selected_env = config
            .selected_env
            .filter(|env| envs.iter().any(|name| name == env))
            .or_else(|| envs.first().cloned());
        self.tree = Some(workspace::scan_tree(&dir));
        self.envs = envs;
        self.selected_env = selected_env;
        self.root = Some(dir);
        self.open_file = None;
        self.editor_text.clear();
        self.parsed = None;
        self.selected_request = None;
        self.results.clear();
        self.dirty = false;
        self.error = None;
        self.status = "工作区已加载".to_owned();
    }

    fn load_file(&mut self, path: PathBuf) {
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                self.editor_text = text;
                self.parsed = Some(parser::parse(&self.editor_text));
                self.open_file = Some(path);
                let has_requests = self
                    .parsed
                    .as_ref()
                    .is_some_and(|parsed| !parsed.requests.is_empty());
                self.selected_request = has_requests.then_some(0);
                self.results.clear();
                self.dirty = false;
                self.error = None;
                self.status = "文件已打开".to_owned();
            }
            Err(err) => self.error = Some(format!("读取失败: {err}")),
        }
    }

    fn reparse(&mut self) {
        self.parsed = Some(parser::parse(&self.editor_text));
        if let Some(index) = self.selected_request {
            let count = self
                .parsed
                .as_ref()
                .map_or(0, |parsed| parsed.requests.len());
            if index >= count {
                self.selected_request = count.checked_sub(1);
            }
        }
    }

    fn save(&mut self) {
        let Some(path) = &self.open_file else {
            return;
        };
        match std::fs::write(path, &self.editor_text) {
            Ok(()) => {
                self.dirty = false;
                self.error = None;
                self.status = format!("已保存 {}", path.display());
            }
            Err(err) => self.error = Some(format!("保存失败: {err}")),
        }
    }

    fn run_selected(&mut self, ctx: &egui::Context) {
        let (Some(root), Some(file), Some(index)) = (
            self.root.clone(),
            self.open_file.clone(),
            self.selected_request,
        ) else {
            return;
        };
        let env = self.selected_env.clone();
        let state = self.state.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.running += 1;
        self.error = None;
        self.status = format!("运行中：第 {} 个请求…", index + 1);
        self.runtime.spawn(async move {
            let root = root.to_string_lossy().to_string();
            let file = file.to_string_lossy().to_string();
            let result =
                httpdeck_lib::run_single_request(&state, &root, &file, index, env.as_deref()).await;
            let _ = tx.send(TaskMsg::Done(result));
            ctx.request_repaint();
        });
    }

    fn run_file(&mut self, ctx: &egui::Context) {
        let (Some(root), Some(file)) = (self.root.clone(), self.open_file.clone()) else {
            return;
        };
        let env = self.selected_env.clone();
        let state = self.state.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.running += 1;
        self.error = None;
        self.status = "运行整个文件…".to_owned();
        self.runtime.spawn(async move {
            let root = root.to_string_lossy().to_string();
            let file = file.to_string_lossy().to_string();
            let result = httpdeck_lib::run_http_file(&state, &root, &file, env.as_deref()).await;
            let _ = tx.send(TaskMsg::FileDone(result));
            ctx.request_repaint();
        });
    }

    fn poll_tasks(&mut self, ctx: &egui::Context) {
        while let Ok(message) = self.rx.try_recv() {
            self.running = self.running.saturating_sub(1);
            match message {
                TaskMsg::Done(Ok(result)) => {
                    self.selected_request = Some(result.request_index);
                    self.status = format!(
                        "{} {} · {} · {} ms",
                        result.method, result.status, result.status_text, result.duration_ms
                    );
                    self.error = None;
                    self.results.insert(result.request_index, result);
                }
                TaskMsg::Done(Err(err)) => {
                    self.status = "执行失败".to_owned();
                    self.error = Some(err);
                }
                TaskMsg::FileDone(Ok(results)) => {
                    self.status = format!("已执行 {} 个请求", results.len());
                    self.error = None;
                    for result in results {
                        self.results.insert(result.request_index, result);
                    }
                }
                TaskMsg::FileDone(Err(err)) => {
                    self.status = "执行失败".to_owned();
                    self.error = Some(err);
                }
            }
        }
        if self.running > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let (save, run) = ctx.input(|input| {
            (
                input.modifiers.command && input.key_pressed(egui::Key::S),
                input.modifiers.command && input.key_pressed(egui::Key::Enter),
            )
        });
        if save {
            self.save();
        }
        if run {
            self.run_selected(ctx);
        }
    }

    fn toolbar(&mut self, root: &mut egui::Ui) -> egui::Rect {
        egui::Panel::top("toolbar").show(root, |ui| {
            let ctx = ui.ctx().clone();
            ui.horizontal(|ui| {
                if ui.button("打开目录").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.load_workspace(dir);
                    }
                }
                if let Some(root) = &self.root {
                    ui.weak(root.display().to_string());
                }
                ui.separator();

                ui.label("环境");
                let selected_text = self.selected_env.clone().unwrap_or_else(|| "无".to_owned());
                egui::ComboBox::from_id_salt("env")
                    .selected_text(selected_text)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(self.selected_env.is_none(), "无")
                            .clicked()
                        {
                            self.selected_env = None;
                        }
                        for env in &self.envs {
                            let selected = self.selected_env.as_deref() == Some(env.as_str());
                            if ui.selectable_label(selected, env).clicked() {
                                self.selected_env = Some(env.clone());
                            }
                        }
                    });
                ui.separator();

                let can_run = self.selected_request.is_some()
                    && self.open_file.is_some()
                    && self.running == 0;
                if ui
                    .add_enabled(can_run, egui::Button::new("运行选中"))
                    .clicked()
                {
                    self.run_selected(&ctx);
                }
                let can_run_file = self.open_file.is_some() && self.running == 0;
                if ui
                    .add_enabled(can_run_file, egui::Button::new("运行文件"))
                    .clicked()
                {
                    self.run_file(&ctx);
                }
                if ui
                    .add_enabled(self.dirty, egui::Button::new("保存"))
                    .clicked()
                {
                    self.save();
                }
                if self.running > 0 {
                    ui.spinner();
                }
                ui.separator();

                if let Some(error) = &self.error {
                    ui.colored_label(egui::Color32::from_rgb(230, 100, 100), error.as_str());
                } else {
                    ui.weak(self.status.as_str());
                }
            });
        })
        .response
        .rect
    }

    fn tree_panel(&mut self, root: &mut egui::Ui) -> egui::Rect {
        egui::Panel::left("tree")
            .resizable(true)
            .default_size(250.0)
            .show(root, |ui| {
                ui.take_available_space();
                ui.horizontal(|ui| {
                    ui.strong("目录");
                    if ui.small_button("刷新").clicked() {
                        if let Some(root) = &self.root {
                            self.tree = Some(workspace::scan_tree(root));
                        }
                    }
                });
                ui.separator();

                let tree = self.tree.take();
                if let Some(node) = &tree {
                    let mut open = None;
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        tree::render(ui, node, self.open_file.as_deref(), &mut open);
                    });
                    if let Some(path) = open {
                        self.load_file(path);
                    }
                }
                self.tree = tree;
            })
            .response
            .rect
    }

    fn requests_panel(&mut self, root: &mut egui::Ui) -> egui::Rect {
        egui::Panel::right("requests")
            .resizable(true)
            .default_size(240.0)
            .show(root, |ui| {
                ui.take_available_space();
                ui.strong("请求");
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let mut clicked = None;
                    if let Some(parsed) = &self.parsed {
                        if parsed.requests.is_empty() {
                            ui.weak("当前文件没有可执行请求");
                        }
                        for (index, request) in parsed.requests.iter().enumerate() {
                            let mut text =
                                format!("{}  {}", request.method, request.display_name());
                            if let Some(result) = self.results.get(&index) {
                                text.push_str(&format!("  [{}]", result.status));
                            }
                            if ui
                                .selectable_label(self.selected_request == Some(index), text)
                                .clicked()
                            {
                                clicked = Some(index);
                            }
                        }
                    } else {
                        ui.weak("打开 .http 文件后显示请求列表");
                    }
                    if let Some(index) = clicked {
                        self.selected_request = Some(index);
                    }
                });
            })
            .response
            .rect
    }

    fn response_panel(&mut self, root: &mut egui::Ui) -> egui::Rect {
        egui::Panel::bottom("response")
            .resizable(true)
            .default_size(300.0)
            .show(root, |ui| {
                ui.take_available_space();
                let result = self
                    .selected_request
                    .and_then(|index| self.results.get(&index));
                response::ui(ui, result, self.error.as_deref());
            })
            .response
            .rect
    }

    fn editor_panel(&mut self, root: &mut egui::Ui) -> egui::Rect {
        egui::CentralPanel::default().show(root, |ui| {
            let Some(path) = self.open_file.clone() else {
                ui.centered_and_justified(|ui| {
                    ui.weak("从左侧目录打开一个 .http 文件");
                });
                return;
            };

            ui.horizontal(|ui| {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("未命名");
                ui.strong(name);
                if self.dirty {
                    ui.colored_label(egui::Color32::from_rgb(230, 170, 70), "未保存");
                }
                if let Some(parsed) = &self.parsed {
                    if !parsed.issues.is_empty() {
                        ui.colored_label(
                            egui::Color32::from_rgb(230, 170, 70),
                            format!("{} 个解析提示", parsed.issues.len()),
                        )
                        .on_hover_text(parsed.issues.join("\n"));
                    }
                }
            });
            ui.separator();

            let theme = ui.visuals().clone();
            let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, _wrap_width: f32| {
                let mut job = http_layout_job(ui, text.as_str(), &theme);
                job.wrap.max_width = f32::INFINITY;
                ui.fonts_mut(|fonts| fonts.layout_job(job))
            };

            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let response = ui.add(
                        egui::TextEdit::multiline(&mut self.editor_text)
                            .code_editor()
                            .desired_width(f32::INFINITY)
                            .desired_rows(30)
                            .layouter(&mut layouter),
                    );
                    if response.changed() {
                        self.dirty = true;
                        self.reparse();
                    }
                });
        })
        .response
        .rect
    }

    /// 绘制整个界面，返回各区域实际矩形（便于测试布局）
    pub fn draw(&mut self, ui: &mut egui::Ui) -> LayoutRects {
        let ctx = ui.ctx().clone();
        self.poll_tasks(&ctx);
        self.handle_shortcuts(&ctx);
        LayoutRects {
            toolbar: self.toolbar(ui),
            tree: self.tree_panel(ui),
            requests: self.requests_panel(ui),
            response: self.response_panel(ui),
            central: self.editor_panel(ui),
        }
    }
}

/// 各面板实际渲染出的矩形（供布局测试断言）
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub struct LayoutRects {
    pub toolbar: egui::Rect,
    pub tree: egui::Rect,
    pub requests: egui::Rect,
    pub response: egui::Rect,
    pub central: egui::Rect,
}

impl eframe::App for HttpDeckApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

/// 读取 `http-client.env.json` / `http-client.private.env.json` 中的环境名
fn read_env_names(root: &Path) -> Vec<String> {
    let mut names = Vec::new();
    for file in ["http-client.env.json", "http-client.private.env.json"] {
        let Ok(text) = std::fs::read_to_string(root.join(file)) else {
            continue;
        };
        let Ok(map) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&text)
        else {
            continue;
        };
        for key in map.keys() {
            if !names.iter().any(|name| name == key) {
                names.push(key.clone());
            }
        }
    }
    names
}

/// 轻量的 .http 语法高亮：方法、URL、头部键、注释、分隔符
fn http_layout_job(ui: &egui::Ui, text: &str, visuals: &egui::Visuals) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    use egui::Color32;

    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
    let dark = visuals.dark_mode;
    let default_color = visuals.text_color();
    let comment_color = if dark {
        Color32::from_rgb(106, 153, 85)
    } else {
        Color32::from_rgb(0, 128, 0)
    };
    let method_color = if dark {
        Color32::from_rgb(86, 156, 214)
    } else {
        Color32::from_rgb(0, 0, 200)
    };
    let url_color = if dark {
        Color32::from_rgb(206, 145, 120)
    } else {
        Color32::from_rgb(163, 21, 21)
    };
    let key_color = if dark {
        Color32::from_rgb(156, 220, 254)
    } else {
        Color32::from_rgb(0, 112, 193)
    };
    let dim_color = Color32::from_gray(120);

    let format = |color| TextFormat {
        font_id: font_id.clone(),
        color,
        ..Default::default()
    };

    let methods = [
        "GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "TRACE", "CONNECT",
    ];

    let mut job = LayoutJob::default();
    let mut in_body = false;

    for line in text.split_inclusive('\n') {
        let indent_len = line.len() - line.trim_start().len();
        let indent = &line[..indent_len];
        let content = line[indent_len..].trim_end_matches(['\r', '\n']);
        let trail = &line[indent_len + content.len()..];

        job.append(indent, 0.0, format(default_color));

        if content.starts_with("###") {
            in_body = false;
            job.append(content, 0.0, format(dim_color));
        } else if content.is_empty() {
            in_body = true;
        } else if content.starts_with('#') || content.starts_with("//") {
            let mut comment = format(comment_color);
            comment.italics = true;
            job.append(content, 0.0, comment);
        } else if !in_body {
            let token_end = content.find(char::is_whitespace).unwrap_or(content.len());
            let method = &content[..token_end];
            if methods
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(method))
            {
                job.append(method, 0.0, format(method_color));
                job.append(&content[token_end..], 0.0, format(url_color));
            } else if let Some(index) = content.find(':') {
                let (key, value) = content.split_at(index + 1);
                job.append(key, 0.0, format(key_color));
                job.append(value, 0.0, format(default_color));
            } else {
                job.append(content, 0.0, format(default_color));
            }
        } else {
            job.append(content, 0.0, format(default_color));
        }

        job.append(trail, 0.0, format(default_color));
    }
    job
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 连续跑多帧，确认四个面板（目录/请求/响应）都占据正常尺寸，
    /// 而不是被内容收缩到最小或被后续面板覆盖。
    #[test]
    fn layout_keeps_all_panels_visible() {
        for (ppp, size) in [(1.0, (1280.0, 820.0)), (1.5, (853.0, 546.0))] {
            let mut app = HttpDeckApp::new();
            let ctx = egui::Context::default();
            ctx.set_pixels_per_point(ppp);
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(size.0, size.1),
                )),
                ..Default::default()
            };
            for frame in 0..3 {
                let mut rects = None;
                let _ = ctx.run_ui(input.clone(), |ui| {
                    rects = Some(app.draw(ui));
                });
                let rects = rects.expect("draw 未执行");
                println!("ppp={ppp} frame {frame}: {rects:?}");
                assert!(
                    rects.tree.width() > 150.0,
                    "目录面板宽度异常: {:?}",
                    rects.tree
                );
                assert!(
                    rects.requests.width() > 150.0,
                    "请求面板宽度异常: {:?}",
                    rects.requests
                );
                assert!(
                    rects.response.height() > 150.0,
                    "响应面板高度异常: {:?}",
                    rects.response
                );
                assert!(
                    rects.central.left() >= rects.tree.right() - 1.0,
                    "中央区域与目录面板重叠: {:?}",
                    rects.central
                );
                assert!(
                    rects.central.right() <= rects.requests.left() + 1.0,
                    "中央区域与请求面板重叠: {:?}",
                    rects.central
                );
                assert!(
                    rects.central.bottom() <= rects.response.top() + 1.0,
                    "中央区域与响应面板重叠: {:?}",
                    rects.central
                );
            }
        }
    }
}
