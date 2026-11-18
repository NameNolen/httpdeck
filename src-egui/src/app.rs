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

/// 文件操作对话框（右键菜单触发）
enum FileDialog {
    New { dir: PathBuf, name: String },
    Rename { path: PathBuf, name: String },
    SaveAs { path: PathBuf, target: String },
    Delete { path: PathBuf },
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
    pinned: Vec<String>,

    open_file: Option<PathBuf>,
    editor_text: String,
    dirty: bool,
    find_open: bool,
    replace_open: bool,
    find_text: String,
    replace_text: String,
    selected_match: Option<usize>,
    scroll_to_char: Option<usize>,
    parsed: Option<ParsedHttpFile>,
    selected_request: Option<usize>,
    results: HashMap<usize, RequestResult>,

    running: usize,
    status: String,
    error: Option<String>,
    file_dialog: Option<FileDialog>,
    dialog_focus_pending: bool,
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
            pinned: Vec::new(),
            open_file: None,
            editor_text: String::new(),
            dirty: false,
            find_open: false,
            replace_open: false,
            find_text: String::new(),
            replace_text: String::new(),
            selected_match: None,
            scroll_to_char: None,
            parsed: None,
            selected_request: None,
            results: HashMap::new(),
            running: 0,
            status: String::new(),
            error: None,
            file_dialog: None,
            dialog_focus_pending: false,
        };
        let initial_root = std::env::args()
            .skip(1)
            .map(PathBuf::from)
            .find(|path| path.is_dir())
            .or_else(|| {
                recent::list()
                    .into_iter()
                    .map(PathBuf::from)
                    .find(|path| path.is_dir())
            });
        if let Some(root) = initial_root {
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
        self.pinned = workspace::valid_pinned(&dir, &config.pinned);
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
        let (save, run, find, replace, escape) = ctx.input(|input| {
            (
                input.modifiers.command && input.key_pressed(egui::Key::S),
                input.modifiers.command && input.key_pressed(egui::Key::Enter),
                input.modifiers.command && input.key_pressed(egui::Key::F),
                input.modifiers.command && input.key_pressed(egui::Key::H),
                input.key_pressed(egui::Key::Escape),
            )
        });
        if save {
            self.save();
        }
        if run {
            self.run_selected(ctx);
        }
        if find {
            self.find_open = true;
            self.selected_match = None;
            ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("find_input")));
        }
        if replace {
            self.find_open = true;
            self.replace_open = true;
            self.selected_match = None;
            ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("find_input")));
            // egui TextEdit 把 Ctrl+H 当作删除前一个字符（Emacs 快捷键），
            // 同帧聚焦的查找框会误删字符，这里消费掉该按键
            ctx.input_mut(|input| {
                input.consume_key(egui::Modifiers::COMMAND, egui::Key::H);
            });
        }
        if escape && self.find_open {
            self.find_open = false;
            self.selected_match = None;
        }
        // 查找打开期间 Enter 全局用于跳转匹配（egui 只有聚焦才绘制选区，
        // 跳转后焦点在编辑器，必须拦截并消费 Enter，否则会被当成编辑输入）
        if self.find_open && self.file_dialog.is_none() {
            let (enter, shift, command) = ctx.input(|input| {
                (
                    input.key_pressed(egui::Key::Enter),
                    input.modifiers.shift,
                    input.modifiers.command,
                )
            });
            if enter && !command {
                self.goto_match(ctx, !shift);
                ctx.input_mut(|input| {
                    input.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
                    input.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter);
                });
            }
        }
    }

    /// 当前查找文本的所有匹配（字节区间）
    fn find_matches(&self) -> Vec<(usize, usize)> {
        if self.find_text.is_empty() {
            return Vec::new();
        }
        self.editor_text
            .match_indices(&self.find_text)
            .map(|(start, text)| (start, start + text.len()))
            .collect()
    }

    /// 选中编辑器中指定字节区间，并聚焦编辑器
    fn select_byte_range(&mut self, ctx: &egui::Context, start: usize, end: usize) {
        let start_char = self.editor_text[..start].chars().count();
        let end_char = self.editor_text[..end].chars().count();
        self.scroll_to_char = Some(start_char);
        let id = egui::Id::new("http_editor");
        if let Some(mut state) = egui::text_edit::TextEditState::load(ctx, id) {
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(start_char),
                    egui::text::CCursor::new(end_char),
                )));
            state.store(ctx, id);
        }
        ctx.memory_mut(|memory| memory.request_focus(id));
        // 焦点切到编辑器后，同一帧的 Enter 事件会被编辑器误当成编辑输入，这里消费掉
        ctx.input_mut(|input| {
            input.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
            input.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter);
        });
    }

    /// 跳到下一个 / 上一个匹配并选中
    fn goto_match(&mut self, ctx: &egui::Context, forward: bool) {
        let matches = self.find_matches();
        if matches.is_empty() {
            return;
        }
        let len = matches.len();
        let index = match (self.selected_match, forward) {
            (Some(current), true) => (current + 1) % len,
            (Some(current), false) => (current + len - 1) % len,
            (None, true) => 0,
            (None, false) => len - 1,
        };
        self.selected_match = Some(index);
        let (start, end) = matches[index];
        self.select_byte_range(ctx, start, end);
    }

    fn replace_current(&mut self, ctx: &egui::Context) {
        if self.find_text.is_empty() {
            return;
        }
        let Some(index) = self.selected_match else {
            self.goto_match(ctx, true);
            return;
        };
        let matches = self.find_matches();
        let Some(&(start, end)) = matches.get(index) else {
            self.selected_match = None;
            return;
        };
        self.editor_text
            .replace_range(start..end, &self.replace_text);
        self.dirty = true;
        self.reparse();
        // 替换后选中同一位置（即下一个匹配）
        let remaining = self.find_matches();
        if remaining.is_empty() {
            self.selected_match = None;
        } else {
            let next = index.min(remaining.len() - 1);
            self.selected_match = Some(next);
            let (start, end) = remaining[next];
            self.select_byte_range(ctx, start, end);
        }
    }

    fn replace_all(&mut self) {
        let count = self.find_matches().len();
        if count == 0 {
            return;
        }
        self.editor_text = self
            .editor_text
            .replace(&self.find_text, &self.replace_text);
        self.dirty = true;
        self.reparse();
        self.selected_match = None;
        self.status = format!("已替换 {count} 处");
    }

    fn refresh_tree(&mut self) {
        if let Some(root) = &self.root {
            self.tree = Some(workspace::scan_tree(root));
        }
    }

    fn set_pinned(&mut self, path: &Path, pinned: bool) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let rel = workspace::rel_path(&root, path);
        let mut config = workspace::load_config(&root);
        if pinned {
            if !config.pinned.contains(&rel) {
                config.pinned.push(rel.clone());
            }
        } else {
            config.pinned.retain(|item| item != &rel);
        }
        match workspace::save_config(&root, &config) {
            Ok(()) => {
                self.pinned = workspace::valid_pinned(&root, &config.pinned);
                self.error = None;
                self.status = if pinned {
                    format!("已置顶 {rel}")
                } else {
                    format!("已取消置顶 {rel}")
                };
            }
            Err(err) => self.error = Some(format!("保存置顶配置失败: {err}")),
        }
    }

    fn handle_tree_action(&mut self, action: tree::TreeAction) {
        let dialog = match action {
            tree::TreeAction::NewFile(dir) => FileDialog::New {
                dir,
                name: "new.http".to_owned(),
            },
            tree::TreeAction::Rename(path) => {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                FileDialog::Rename { path, name }
            }
            tree::TreeAction::SaveAs(path) => {
                let target = duplicate_target(&path)
                    .map(|target| target.to_string_lossy().into_owned())
                    .unwrap_or_default();
                FileDialog::SaveAs { path, target }
            }
            tree::TreeAction::Delete(path) => FileDialog::Delete { path },
            tree::TreeAction::SetPinned(path, pinned) => {
                self.set_pinned(&path, pinned);
                return;
            }
            tree::TreeAction::Duplicate(path) => {
                match duplicate_file(&path) {
                    Ok(target) => {
                        self.refresh_tree();
                        self.error = None;
                        self.status = format!("已复制为 {}", target.display());
                    }
                    Err(err) => self.error = Some(err),
                }
                return;
            }
        };
        self.file_dialog = Some(dialog);
        self.dialog_focus_pending = true;
    }

    fn render_file_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.file_dialog else {
            return;
        };
        let title = match dialog {
            FileDialog::New { .. } => "新建文件",
            FileDialog::Rename { .. } => "重命名",
            FileDialog::SaveAs { .. } => "另存为",
            FileDialog::Delete { .. } => "删除文件",
        };
        let mut confirm = false;
        let mut cancel = false;
        egui::Window::new(title)
            .id(egui::Id::new("file_dialog"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                match dialog {
                    FileDialog::New { name, .. } | FileDialog::Rename { name, .. } => {
                        ui.label("文件名（自动补 .http 后缀）：");
                        ui.add(
                            egui::TextEdit::singleline(name)
                                .id(egui::Id::new("file_dialog_input"))
                                .desired_width(260.0),
                        );
                    }
                    FileDialog::SaveAs { target, .. } => {
                        ui.label("目标路径：");
                        ui.add(
                            egui::TextEdit::singleline(target)
                                .id(egui::Id::new("file_dialog_input"))
                                .desired_width(360.0),
                        );
                    }
                    FileDialog::Delete { path } => {
                        ui.label(format!("确定删除 {} 吗？", path.display()));
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let label = if matches!(dialog, FileDialog::Delete { .. }) {
                        "删除"
                    } else {
                        "确定"
                    };
                    if ui.button(label).clicked() {
                        confirm = true;
                    }
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                });
            });
        // 对话框首帧渲染完成后聚焦输入框并全选默认文本，方便直接输入覆盖
        if self.dialog_focus_pending {
            self.dialog_focus_pending = false;
            let id = egui::Id::new("file_dialog_input");
            ctx.memory_mut(|memory| memory.request_focus(id));
            let text_len = match dialog {
                FileDialog::New { name, .. } | FileDialog::Rename { name, .. } => {
                    name.chars().count()
                }
                FileDialog::SaveAs { target, .. } => target.chars().count(),
                FileDialog::Delete { .. } => 0,
            };
            if text_len > 0 {
                if let Some(mut state) = egui::text_edit::TextEditState::load(ctx, id) {
                    state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::two(
                            egui::text::CCursor::new(0),
                            egui::text::CCursor::new(text_len),
                        )));
                    state.store(ctx, id);
                }
            }
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Enter)) {
            confirm = true;
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }
        if cancel {
            self.file_dialog = None;
            return;
        }
        if !confirm {
            return;
        }
        let Some(dialog) = self.file_dialog.take() else {
            return;
        };
        let mut open = None;
        let result = match dialog {
            FileDialog::New { dir, name } => create_file(&dir, &name).map(|path| {
                open = Some(path.clone());
                format!("已新建 {}", path.display())
            }),
            FileDialog::Rename { path, name } => rename_file(&path, &name).map(|target| {
                if self.open_file.as_deref() == Some(path.as_path()) {
                    self.open_file = Some(target.clone());
                }
                format!("已重命名为 {}", target.display())
            }),
            FileDialog::SaveAs { path, target } => {
                save_as_file(&path, Path::new(&target)).map(|()| format!("已另存为 {target}"))
            }
            FileDialog::Delete { path } => delete_file(&path).map(|()| {
                if self.open_file.as_deref() == Some(path.as_path()) {
                    self.open_file = None;
                    self.editor_text.clear();
                    self.parsed = None;
                    self.selected_request = None;
                    self.results.clear();
                    self.dirty = false;
                }
                format!("已删除 {}", path.display())
            }),
        };
        match result {
            Ok(message) => {
                self.refresh_tree();
                self.error = None;
                self.status = message;
                if let Some(path) = open {
                    self.load_file(path);
                }
            }
            Err(err) => self.error = Some(err),
        }
    }

    fn toolbar(&mut self, root: &mut egui::Ui) -> egui::Rect {
        egui::Panel::top("toolbar")
            .show(root, |ui| {
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
                    let selected_text =
                        self.selected_env.clone().unwrap_or_else(|| "无".to_owned());
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
                let mut action = None;
                let mut open = None;
                if let (Some(workspace_root), Some(node)) = (self.root.as_ref(), tree.as_ref()) {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        if !self.pinned.is_empty() {
                            ui.strong("置顶常用");
                            for rel in &self.pinned {
                                let path = workspace_root.join(rel);
                                let node = FileNode {
                                    name: path
                                        .file_name()
                                        .map(|name| name.to_string_lossy().into_owned())
                                        .unwrap_or_else(|| rel.clone()),
                                    path: path.to_string_lossy().into_owned(),
                                    rel_path: rel.clone(),
                                    is_dir: false,
                                    children: Vec::new(),
                                };
                                tree::render_file(
                                    ui,
                                    &node,
                                    true,
                                    self.open_file.as_deref(),
                                    &mut open,
                                    &mut action,
                                );
                            }
                            ui.separator();
                        }
                        tree::render(
                            ui,
                            node,
                            &self.pinned,
                            self.open_file.as_deref(),
                            &mut open,
                            &mut action,
                        );
                    });
                }
                self.tree = tree;
                if let Some(path) = open {
                    self.load_file(path);
                }
                // 注意要在恢复 self.tree 之后再处理动作，
                // 否则动作里的 refresh_tree 会被上面的旧树覆盖
                if let Some(action) = action {
                    self.handle_tree_action(action);
                }
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
        egui::CentralPanel::default()
            .show(root, |ui| {
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

                if self.find_open {
                    let ctx = ui.ctx().clone();
                    ui.horizontal(|ui| {
                        ui.label("查找");
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut self.find_text)
                                .id(egui::Id::new("find_input"))
                                .desired_width(160.0),
                        );
                        if response.changed() {
                            self.selected_match = None;
                        }
                        if self.replace_open {
                            ui.label("替换");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.replace_text)
                                    .desired_width(140.0),
                            );
                        }
                        let count = self.find_matches().len();
                        if !self.find_text.is_empty() {
                            let current =
                                self.selected_match.map(|i| (i + 1).min(count)).unwrap_or(0);
                            ui.weak(format!("{current}/{count}"));
                        }
                        if ui.button("下一个").clicked() {
                            self.goto_match(&ctx, true);
                        }
                        if ui.button("上一个").clicked() {
                            self.goto_match(&ctx, false);
                        }
                        if self.replace_open {
                            if ui.button("替换").clicked() {
                                self.replace_current(&ctx);
                            }
                            if ui.button("全部替换").clicked() {
                                self.replace_all();
                            }
                        }
                        if ui.button("关闭").clicked() {
                            self.find_open = false;
                            self.selected_match = None;
                        }
                    });
                    ui.separator();
                }

                let theme = ui.visuals().clone();
                let mut layouter =
                    |ui: &egui::Ui, text: &dyn egui::TextBuffer, _wrap_width: f32| {
                        let mut job = http_layout_job(ui, text.as_str(), &theme);
                        job.wrap.max_width = f32::INFINITY;
                        ui.fonts_mut(|fonts| fonts.layout_job(job))
                    };

                // 行号：TextEdit 左边距留白，按 galley 行位置用 painter 绘制，天然逐行对齐
                let line_count = self.editor_text.lines().count().max(1);
                let font_id = ui
                    .style()
                    .text_styles
                    .get(&egui::TextStyle::Monospace)
                    .cloned()
                    .unwrap_or_else(|| egui::FontId::monospace(14.0));
                let digit_w = ui.fonts_mut(|fonts| fonts.glyph_width(&font_id, '0'));
                let gutter_w = (line_count.to_string().len() as f32 * digit_w + 14.0).ceil();

                let mut paint_info = None;
                let scroll_output = egui::ScrollArea::both()
                    .id_salt("editor")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let out = egui::TextEdit::multiline(&mut self.editor_text)
                            .id(egui::Id::new("http_editor"))
                            .code_editor()
                            .desired_width(f32::INFINITY)
                            .desired_rows(30)
                            .margin(egui::Margin {
                                left: gutter_w as i8,
                                right: 4,
                                top: 4,
                                bottom: 4,
                            })
                            .layouter(&mut layouter)
                            .show(ui);
                        if out.response.changed() {
                            self.dirty = true;
                            self.reparse();
                        }
                        // 查找跳转是在 widget 运行前改的状态，TextEdit 检测不到选区变化，
                        // 不会自动滚动，这里手动把匹配位置滚动到可视区域
                        if let Some(char_index) = self.scroll_to_char.take() {
                            let cursor_rect = out
                                .galley
                                .pos_from_cursor(egui::text::CCursor::new(char_index))
                                .translate(out.galley_pos.to_vec2());
                            ui.scroll_to_rect(cursor_rect, None);
                        }
                        paint_info = Some((out.galley.clone(), out.galley_pos));
                    });

                if let Some((galley, galley_pos)) = paint_info {
                    let viewport = scroll_output.inner_rect;
                    let painter = ui.painter().with_clip_rect(viewport);
                    let gutter_rect = egui::Rect::from_min_size(
                        viewport.min,
                        egui::vec2(gutter_w, viewport.height()),
                    );
                    // 覆盖水平滚动进入行号列的文本
                    painter.rect_filled(gutter_rect, 0.0, ui.visuals().extreme_bg_color);
                    for (index, row) in galley.rows.iter().enumerate() {
                        let row_rect = row.rect();
                        let y = galley_pos.y + row_rect.min.y;
                        if y + row_rect.height() < viewport.top() || y > viewport.bottom() {
                            continue;
                        }
                        painter.text(
                            egui::pos2(viewport.left() + gutter_w - 8.0, y),
                            egui::Align2::RIGHT_TOP,
                            (index + 1).to_string(),
                            font_id.clone(),
                            ui.visuals().weak_text_color(),
                        );
                    }
                }
            })
            .response
            .rect
    }

    /// 绘制整个界面，返回各区域实际矩形（便于测试布局）
    pub fn draw(&mut self, ui: &mut egui::Ui) -> LayoutRects {
        let ctx = ui.ctx().clone();
        self.poll_tasks(&ctx);
        self.handle_shortcuts(&ctx);
        let rects = LayoutRects {
            toolbar: self.toolbar(ui),
            tree: self.tree_panel(ui),
            requests: self.requests_panel(ui),
            response: self.response_panel(ui),
            central: self.editor_panel(ui),
        };
        self.render_file_dialog(&ctx);
        rects
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

/// 文件名去空白并自动补 `.http` 后缀
fn http_file_name(name: &str) -> String {
    let name = name.trim();
    if name.to_ascii_lowercase().ends_with(".http") {
        name.to_owned()
    } else {
        format!("{name}.http")
    }
}

fn create_file(dir: &Path, name: &str) -> Result<PathBuf, String> {
    if name.trim().is_empty() {
        return Err("文件名不能为空".to_owned());
    }
    let target = dir.join(http_file_name(name));
    if target.exists() {
        return Err(format!("文件已存在: {}", target.display()));
    }
    std::fs::write(&target, "### 新请求\nGET https://example.com\n")
        .map_err(|err| format!("新建文件失败: {err}"))?;
    Ok(target)
}

fn rename_file(path: &Path, name: &str) -> Result<PathBuf, String> {
    if name.trim().is_empty() {
        return Err("文件名不能为空".to_owned());
    }
    let dir = path
        .parent()
        .ok_or_else(|| "无法确定文件所在目录".to_owned())?;
    let target = dir.join(http_file_name(name));
    if target == path {
        return Ok(target);
    }
    if target.exists() {
        return Err(format!("文件已存在: {}", target.display()));
    }
    std::fs::rename(path, &target).map_err(|err| format!("重命名失败: {err}"))?;
    Ok(target)
}

fn delete_file(path: &Path) -> Result<(), String> {
    std::fs::remove_file(path).map_err(|err| format!("删除失败: {err}"))
}

/// 另存为：把源文件原样复制到指定位置
fn save_as_file(path: &Path, target: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Err(format!("文件不存在: {}", path.display()));
    }
    std::fs::copy(path, target).map_err(|err| format!("另存为失败: {err}"))?;
    Ok(())
}

fn duplicate_file(path: &Path) -> Result<PathBuf, String> {
    let target = duplicate_target(path)?;
    std::fs::copy(path, &target)
        .map_err(|err| format!("复制文件失败 {}: {err}", target.display()))?;
    Ok(target)
}

/// 生成「同名 + 序号」的不重复路径：api.http -> api1.http -> api2.http
fn duplicate_target(source: &Path) -> Result<PathBuf, String> {
    let dir = source
        .parent()
        .ok_or_else(|| "无法确定文件所在目录".to_owned())?;
    let stem = source
        .file_stem()
        .and_then(|item| item.to_str())
        .unwrap_or("file");
    let ext = source
        .extension()
        .map(|item| format!(".{}", item.to_string_lossy()))
        .unwrap_or_default();
    // 已经没有序号的文件从 1 开始，已有序号的文件在其序号上递增
    let (base, start) = match stem.rfind(|c: char| !c.is_ascii_digit()) {
        Some(index) => {
            let (prefix, digits) = stem.split_at(index + 1);
            match digits.parse::<u32>() {
                Ok(number) => (prefix.to_string(), number + 1),
                Err(_) => (stem.to_string(), 1),
            }
        }
        None => (stem.to_string(), 1),
    };
    let mut number = start.max(1);
    loop {
        let candidate = dir.join(format!("{base}{number}{ext}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
        number += 1;
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
                let mut output = ctx.run_ui(input.clone(), |ui| {
                    rects = Some(app.draw(ui));
                });
                output.textures_delta.clear();
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

    /// 查找打开时 Enter 应循环选中匹配，且不能被编辑器当成输入修改文本
    #[test]
    fn find_enter_selects_match_without_editing() {
        let mut app = HttpDeckApp::new();
        app.open_file = Some(PathBuf::from("test.http"));
        app.editor_text = "GET a\nGET b\n".to_owned();
        app.find_open = true;
        app.find_text = "GET".to_owned();
        let ctx = egui::Context::default();
        for expected in [Some(0), Some(1), Some(0)] {
            let mut input = egui::RawInput::default();
            input.events.push(egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            });
            let mut output = ctx.run_ui(input, |ui| {
                app.draw(ui);
            });
            output.textures_delta.clear();
            assert_eq!(app.selected_match, expected);
            assert_eq!(app.editor_text, "GET a\nGET b\n");
        }
    }

    /// 替换当前 / 全部替换
    #[test]
    fn replace_current_and_all() {
        let mut app = HttpDeckApp::new();
        app.open_file = Some(PathBuf::from("test.http"));
        app.editor_text = "GET a\nGET b\n".to_owned();
        app.find_open = true;
        app.replace_open = true;
        app.find_text = "GET".to_owned();
        app.replace_text = "POST".to_owned();
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            app.draw(ui);
        });
        output.textures_delta.clear();
        app.goto_match(&ctx, true);
        app.replace_current(&ctx);
        assert_eq!(app.editor_text, "POST a\nGET b\n");
        app.replace_all();
        assert_eq!(app.editor_text, "POST a\nPOST b\n");
        assert!(app.dirty);
    }

    /// 新建 / 复制 / 另存为 / 重命名 / 删除的磁盘行为
    #[test]
    fn file_operations_work() {
        let dir =
            std::env::temp_dir().join(format!("httpdeck-egui-fileops-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let created = create_file(&dir, "demo").unwrap();
        assert!(created.ends_with("demo.http"));
        assert!(created.exists());
        assert!(create_file(&dir, "demo").is_err(), "重名应报错");

        let copy = duplicate_file(&created).unwrap();
        assert!(copy.ends_with("demo1.http"));
        assert!(copy.exists());
        let copy2 = duplicate_file(&copy).unwrap();
        assert!(copy2.ends_with("demo2.http"), "已有序号应递增: {copy2:?}");

        let export = dir.join("export.http");
        save_as_file(&copy, &export).unwrap();
        assert_eq!(
            std::fs::read_to_string(&copy).unwrap(),
            std::fs::read_to_string(&export).unwrap()
        );

        let renamed = rename_file(&created, "renamed").unwrap();
        assert!(renamed.ends_with("renamed.http"));
        assert!(!created.exists());
        assert!(renamed.exists());

        delete_file(&renamed).unwrap();
        assert!(!renamed.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 置顶与取消置顶应同步界面状态并持久化到工作区配置
    #[test]
    fn pin_file_persists() {
        let dir = std::env::temp_dir().join(format!("httpdeck-egui-pin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("demo.http");
        std::fs::write(&file, "GET https://example.com\n").unwrap();
        let mut app = HttpDeckApp::new();
        app.root = Some(dir.clone());

        app.handle_tree_action(tree::TreeAction::SetPinned(file.clone(), true));
        assert_eq!(app.pinned, vec!["demo.http"]);
        assert_eq!(workspace::load_config(&dir).pinned, vec!["demo.http"]);

        app.handle_tree_action(tree::TreeAction::SetPinned(file, false));
        assert!(app.pinned.is_empty());
        assert!(workspace::load_config(&dir).pinned.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 对话框 Enter 确认执行操作，Escape 取消
    #[test]
    fn file_dialog_enter_confirms_and_escape_cancels() {
        let dir = std::env::temp_dir().join(format!("httpdeck-egui-dialog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = HttpDeckApp::new();
        app.file_dialog = Some(FileDialog::New {
            dir: dir.clone(),
            name: "demo".to_owned(),
        });
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        let mut output = ctx.run_ui(input, |ui| {
            app.draw(ui);
        });
        output.textures_delta.clear();
        assert!(app.file_dialog.is_none(), "Enter 应对话框关闭");
        let created = dir.join("demo.http");
        assert!(created.exists(), "Enter 应创建文件");
        assert_eq!(
            app.open_file.as_deref(),
            Some(created.as_path()),
            "新文件应被打开"
        );

        app.file_dialog = Some(FileDialog::Delete {
            path: created.clone(),
        });
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        let mut output = ctx.run_ui(input, |ui| {
            app.draw(ui);
        });
        output.textures_delta.clear();
        assert!(app.file_dialog.is_none(), "Escape 应对话框关闭");
        assert!(created.exists(), "Escape 取消不应删除文件");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
