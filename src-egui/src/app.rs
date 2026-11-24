use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use httpdeck_lib::models::{
    FileNode, FileRunResult, HistoryEntry, KeyValue, McpStatus, RequestResult, RequestSummary,
};
use httpdeck_lib::parser::{self, ParsedHttpFile};
use httpdeck_lib::{history, recent, workspace};

use crate::{response, tree};

enum TaskMsg {
    Done {
        path: String,
        index: usize,
        result: Result<RequestResult, String>,
    },
    FileDone {
        path: String,
        result: Result<Vec<RequestResult>, String>,
    },
    FolderDone {
        result: Result<Vec<FileRunResult>, String>,
    },
    McpDone(Result<McpStatus, String>),
}

/// 文件操作对话框（右键菜单触发）
enum FileDialog {
    New { dir: PathBuf, name: String },
    Rename { path: PathBuf, name: String },
    SaveAs { path: PathBuf, target: String },
    Delete { path: PathBuf },
}

/// 右侧面板当前显示请求列表还是执行历史
#[derive(Clone, Copy, PartialEq, Eq)]
enum RequestsTab {
    Requests,
    History,
    Globals,
}

/// 跨重启保存的界面状态
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
struct UiState {
    #[serde(default, rename = "responseHeight")]
    response_height: Option<f32>,
}

/// 一个已打开文件的标签；切换标签时用它保存 / 恢复编辑器状态
struct Tab {
    path: PathBuf,
    text: String,
    dirty: bool,
    selected_request: Option<usize>,
    results: HashMap<usize, RequestResult>,
}

/// 一个已加载的工作区文件夹；可同时加载多个，同名文件夹也能共存
struct Workspace {
    root: PathBuf,
    tree: FileNode,
    envs: Vec<String>,
    selected_env: Option<String>,
    pinned: Vec<String>,
}

pub struct HttpDeckApp {
    state: Arc<httpdeck_lib::AppState>,
    runtime: tokio::runtime::Runtime,
    tx: Sender<TaskMsg>,
    rx: Receiver<TaskMsg>,

    /// 已加载的工作区列表，目录树中每个工作区一个根节点
    workspaces: Vec<Workspace>,
    /// 历史/变量面板当前对应的工作区，用于检测活动工作区切换
    panel_root: Option<PathBuf>,

    /// 已打开的文件标签，`active_tab` 指向当前显示的那个
    tabs: Vec<Tab>,
    active_tab: Option<usize>,
    requests_tab: RequestsTab,
    /// 当前工作区的请求历史（新的在前）
    history: Vec<HistoryEntry>,
    history_search: String,
    /// 脚本写入的 client.global 变量快照
    globals: Vec<KeyValue>,
    show_mcp: bool,
    mcp_busy: bool,
    mcp_status: Option<McpStatus>,
    show_settings: bool,
    settings_proxy: String,
    settings_cert: String,
    settings_key: String,
    ui_state: UiState,
    response_height: f32,
    ui_state_dirty: bool,
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
    /// 左侧目录中已展开请求列表的文件（绝对路径）
    expanded_files: HashSet<String>,
    /// 懒解析得到的文件请求列表，展开时才填充
    file_requests: HashMap<String, Vec<RequestSummary>>,

    running: usize,
    /// 正在执行的单个请求（文件绝对路径, 请求索引），用于界面显示执行中状态
    running_requests: HashSet<(String, usize)>,
    /// 正在整体执行的文件（绝对路径）
    running_files: HashSet<String>,
    /// 浮动请求面板：将请求列表从右侧面板中分离出来，作为可拖动的小面板常驻桌面
    floating_requests: bool,
    /// 浮动请求面板：完整模式 vs 精简模式
    floating_full_mode: bool,
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
        let ui_state = load_ui_state_from(&ui_state_path());
        let response_height = ui_state
            .response_height
            .unwrap_or(300.0)
            .clamp(120.0, 1400.0);
        let mut app = Self {
            state: Arc::new(httpdeck_lib::AppState::default()),
            runtime,
            tx,
            rx,
            workspaces: Vec::new(),
            panel_root: None,
            tabs: Vec::new(),
            active_tab: None,
            requests_tab: RequestsTab::Requests,
            history: Vec::new(),
            history_search: String::new(),
            globals: Vec::new(),
            show_mcp: false,
            mcp_busy: false,
            mcp_status: None,
            show_settings: false,
            settings_proxy: String::new(),
            settings_cert: String::new(),
            settings_key: String::new(),
            ui_state,
            response_height,
            ui_state_dirty: false,
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
            expanded_files: HashSet::new(),
            file_requests: HashMap::new(),
            running: 0,
            running_requests: HashSet::new(),
            running_files: HashSet::new(),
            floating_requests: false,
            floating_full_mode: false,
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

    /// 加载一个工作区文件夹；已加载的不会重复添加，多个工作区可同时存在
    fn load_workspace(&mut self, dir: PathBuf) {
        if self.workspace_index_for(&dir).is_some() {
            self.status = format!("工作区已加载 {}", dir.display());
            return;
        }
        recent::record(&dir);
        let config = workspace::load_config(&dir);
        let envs = read_env_names(&dir);
        let selected_env = config
            .selected_env
            .filter(|env| envs.iter().any(|name| name == env))
            .or_else(|| envs.first().cloned());
        let tree = workspace::scan_tree(&dir);
        let pinned = workspace::valid_pinned(&dir, &config.pinned);
        self.workspaces.push(Workspace {
            root: dir,
            tree,
            envs,
            selected_env,
            pinned,
        });
        self.error = None;
        self.status = "工作区已加载".to_owned();
        self.sync_panels();
    }

    /// 从列表中移除工作区；已打开的标签保留，避免丢失未保存修改
    fn close_workspace(&mut self, root: &Path) {
        self.workspaces.retain(|ws| ws.root != root);
        self.status = format!("已关闭工作区 {}", root.display());
        self.sync_panels();
    }

    fn workspace_index_for(&self, path: &Path) -> Option<usize> {
        self.workspaces
            .iter()
            .position(|ws| path.starts_with(&ws.root))
    }

    /// 活动工作区：优先当前打开文件所属的工作区，否则最近加载的那个
    fn active_workspace_index(&self) -> Option<usize> {
        if let Some(file) = &self.open_file {
            if let Some(index) = self.workspace_index_for(file) {
                return Some(index);
            }
        }
        self.workspaces.len().checked_sub(1)
    }

    fn active_root(&self) -> Option<PathBuf> {
        self.active_workspace_index()
            .map(|index| self.workspaces[index].root.clone())
    }

    /// 运行请求时的工作区根目录与环境，按文件归属解析
    fn run_context(&self, file: &Path) -> Option<(PathBuf, Option<String>)> {
        let index = self
            .workspace_index_for(file)
            .or_else(|| self.active_workspace_index())?;
        let ws = &self.workspaces[index];
        Some((ws.root.clone(), ws.selected_env.clone()))
    }

    /// 指定文件的某个请求是否正在执行
    fn request_is_running(&self, file: &Path, index: usize) -> bool {
        self.running_requests
            .contains(&(file.to_string_lossy().to_string(), index))
    }

    /// 活动工作区变化时刷新历史 / 变量面板
    fn sync_panels(&mut self) {
        let root = self.active_root();
        if self.panel_root == root {
            return;
        }
        self.panel_root = root;
        self.refresh_history();
        self.refresh_globals();
    }

    fn load_file(&mut self, path: PathBuf) {
        // 文件已在某个标签中打开时直接切换，避免重复读取覆盖未保存内容
        if let Some(index) = self.tabs.iter().position(|tab| tab.path == path) {
            self.activate_tab(index);
            return;
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                self.sync_active_tab();
                let parsed = parser::parse(&text);
                let has_requests = !parsed.requests.is_empty();
                self.tabs.push(Tab {
                    path: path.clone(),
                    text: text.clone(),
                    dirty: false,
                    selected_request: has_requests.then_some(0),
                    results: HashMap::new(),
                });
                self.active_tab = Some(self.tabs.len() - 1);
                self.editor_text = text;
                self.parsed = Some(parsed);
                self.open_file = Some(path);
                self.selected_request = has_requests.then_some(0);
                self.results.clear();
                self.dirty = false;
                self.error = None;
                self.status = "文件已打开".to_owned();
            }
            Err(err) => self.error = Some(format!("读取失败: {err}")),
        }
    }

    /// 把当前编辑器状态写回它所属的标签
    fn sync_active_tab(&mut self) {
        let Some(index) = self.active_tab else {
            return;
        };
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        tab.text = self.editor_text.clone();
        tab.dirty = self.dirty;
        tab.selected_request = self.selected_request;
        tab.results = self.results.clone();
    }

    /// 切换到第 index 个标签，恢复该标签的文本、修改状态与运行结果
    fn activate_tab(&mut self, index: usize) {
        if self.active_tab == Some(index) {
            return;
        }
        self.sync_active_tab();
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let text = tab.text.clone();
        let path = tab.path.clone();
        let selected = tab.selected_request;
        let results = tab.results.clone();
        let dirty = tab.dirty;
        self.active_tab = Some(index);
        self.parsed = Some(parser::parse(&text));
        self.editor_text = text;
        self.open_file = Some(path);
        self.selected_request = selected.or_else(|| {
            self.parsed
                .as_ref()
                .is_some_and(|parsed| !parsed.requests.is_empty())
                .then_some(0)
        });
        if let Some(index) = self.selected_request {
            let count = self
                .parsed
                .as_ref()
                .map_or(0, |parsed| parsed.requests.len());
            if index >= count {
                self.selected_request = count.checked_sub(1);
            }
        }
        self.results = results;
        self.dirty = dirty;
        self.scroll_to_char = None;
        self.error = None;
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

    /// 关闭当前文件；有未保存修改时提示先保存，避免误丢内容
    fn close_file(&mut self) {
        if self.dirty {
            self.error = Some("有未保存的修改，请先保存（Ctrl+S）再关闭".to_owned());
            return;
        }
        let name = self
            .open_file
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let closed = self.active_tab;
        if let Some(index) = closed {
            self.tabs.remove(index);
        }
        self.active_tab = None;
        self.open_file = None;
        self.editor_text.clear();
        self.parsed = None;
        self.selected_request = None;
        self.results.clear();
        self.scroll_to_char = None;
        self.error = None;
        // 关闭后优先激活相邻标签
        if !self.tabs.is_empty() {
            let next = closed.unwrap_or(0).min(self.tabs.len() - 1);
            self.activate_tab(next);
        }
        self.status = if name.is_empty() {
            "文件已关闭".to_owned()
        } else {
            format!("已关闭 {name}")
        };
    }

    /// 文件被删除 / 改名后同步标签路径；`None` 表示删除并关闭对应标签
    fn update_tab_path(&mut self, old: &Path, new: Option<PathBuf>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.path == old) else {
            return;
        };
        match new {
            Some(path) => {
                if self.active_tab == Some(index) {
                    self.open_file = Some(path.clone());
                }
                self.tabs[index].path = path;
            }
            None => {
                let was_active = self.active_tab == Some(index);
                self.tabs.remove(index);
                if was_active {
                    self.active_tab = None;
                    self.open_file = None;
                    self.editor_text.clear();
                    self.parsed = None;
                    self.selected_request = None;
                    self.results.clear();
                    self.dirty = false;
                    if !self.tabs.is_empty() {
                        let next = index.min(self.tabs.len() - 1);
                        self.activate_tab(next);
                    }
                } else if let Some(active) = self.active_tab {
                    if index < active {
                        self.active_tab = Some(active - 1);
                    }
                }
            }
        }
    }

    fn run_selected(&mut self, ctx: &egui::Context) {
        let (Some(file), Some(index)) = (self.open_file.clone(), self.selected_request) else {
            return;
        };
        let Some((root, env)) = self.run_context(&file) else {
            return;
        };
        let state = self.state.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let file_key = file.to_string_lossy().to_string();
        self.running += 1;
        self.running_requests.insert((file_key.clone(), index));
        self.error = None;
        self.status = format!("运行中：第 {} 个请求…", index + 1);
        self.runtime.spawn(async move {
            let root = root.to_string_lossy().to_string();
            let result =
                httpdeck_lib::run_single_request(&state, &root, &file_key, index, env.as_deref())
                    .await;
            let _ = tx.send(TaskMsg::Done {
                path: file_key,
                index,
                result,
            });
            ctx.request_repaint();
        });
    }

    fn run_file(&mut self, ctx: &egui::Context) {
        let Some(file) = self.open_file.clone() else {
            return;
        };
        let Some((root, env)) = self.run_context(&file) else {
            return;
        };
        let state = self.state.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let file_key = file.to_string_lossy().to_string();
        self.running += 1;
        self.running_files.insert(file_key.clone());
        self.error = None;
        self.status = "运行整个文件…".to_owned();
        self.runtime.spawn(async move {
            let root = root.to_string_lossy().to_string();
            let result =
                httpdeck_lib::run_http_file(&state, &root, &file_key, env.as_deref()).await;
            let _ = tx.send(TaskMsg::FileDone {
                path: file_key,
                result,
            });
            ctx.request_repaint();
        });
    }

    /// 递归运行目录下所有 .http 文件
    fn run_folder(&mut self, ctx: &egui::Context, dir: PathBuf) {
        let Some((root, env)) = self.run_context(&dir) else {
            return;
        };
        let state = self.state.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.running += 1;
        self.error = None;
        self.status = format!("运行目录 {}…", dir.display());
        self.runtime.spawn(async move {
            let root = root.to_string_lossy().to_string();
            let folder = dir.to_string_lossy().to_string();
            let result =
                httpdeck_lib::run_http_folder(&state, &root, &folder, env.as_deref()).await;
            let _ = tx.send(TaskMsg::FolderDone { result });
            ctx.request_repaint();
        });
    }

    fn poll_tasks(&mut self, ctx: &egui::Context) {
        let mut received = false;
        while let Ok(message) = self.rx.try_recv() {
            received = true;
            self.running = self.running.saturating_sub(1);
            match message {
                TaskMsg::Done {
                    path,
                    index,
                    result,
                } => {
                    self.running_requests.remove(&(path.clone(), index));
                    match result {
                        Ok(result) => {
                            let message = format!(
                                "{} {} · {} · {} ms",
                                result.method,
                                result.status,
                                result.status_text,
                                result.duration_ms
                            );
                            self.store_results(&path, vec![result], Some(&message), true);
                        }
                        Err(err) => {
                            self.status = "执行失败".to_owned();
                            self.error = Some(err);
                        }
                    }
                }
                TaskMsg::FileDone { path, result } => {
                    self.running_files.remove(&path);
                    match result {
                        Ok(results) => {
                            let message = format!("已执行 {} 个请求", results.len());
                            self.store_results(&path, results, Some(&message), false);
                        }
                        Err(err) => {
                            self.status = "执行失败".to_owned();
                            self.error = Some(err);
                        }
                    }
                }
                TaskMsg::McpDone(result) => {
                    self.mcp_busy = false;
                    match result {
                        Ok(status) => {
                            let running = status.running;
                            self.mcp_status = Some(status);
                            self.error = None;
                            self.status = if running {
                                "MCP Server 已启动".to_owned()
                            } else {
                                "MCP Server 已停止".to_owned()
                            };
                        }
                        Err(err) => self.error = Some(err),
                    }
                }
                TaskMsg::FolderDone { result } => match result {
                    Ok(files) => {
                        let file_count = files.len();
                        let request_count: usize =
                            files.iter().map(|file| file.results.len()).sum();
                        let failed = files.iter().filter(|file| file.error.is_some()).count();
                        for file in files {
                            self.store_results(&file.file, file.results, None, false);
                        }
                        self.error = None;
                        self.status = folder_run_summary(file_count, request_count, failed);
                    }
                    Err(err) => {
                        self.status = "执行失败".to_owned();
                        self.error = Some(err);
                    }
                },
            }
        }
        if received {
            self.refresh_history();
            self.refresh_globals();
        }
        if self.running > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// 把运行结果写入发起请求的文件标签；用户运行期间切换标签时结果也不会串台
    fn store_results(
        &mut self,
        path: &str,
        results: Vec<RequestResult>,
        message: Option<&str>,
        select_result: bool,
    ) {
        let selected = select_result
            .then(|| results.last().map(|result| result.request_index))
            .flatten();
        let active = self
            .open_file
            .as_ref()
            .is_some_and(|open| open.to_string_lossy().as_ref() == path);
        if active {
            if let Some(index) = selected {
                self.selected_request = Some(index);
            }
            for result in results {
                self.results.insert(result.request_index, result);
            }
            self.error = None;
            if let Some(message) = message {
                self.status = message.to_owned();
            }
            return;
        }
        if let Some(tab) = self
            .tabs
            .iter_mut()
            .find(|tab| tab.path.to_string_lossy().as_ref() == path)
        {
            for result in results {
                tab.results.insert(result.request_index, result);
            }
            self.error = None;
            if let Some(message) = message {
                self.status = format!("{message}（后台标签）");
            }
            return;
        }
        // 标签已关闭时结果无处可放，仅更新状态栏
        if let Some(message) = message {
            self.status = message.to_owned();
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

    /// 点击请求列表时，把编辑器滚动并定位到该请求所在行
    fn jump_to_request(&mut self, ctx: &egui::Context, index: usize) {
        let line = self
            .parsed
            .as_ref()
            .and_then(|parsed| parsed.requests.get(index))
            .map(|request| request.line)
            .unwrap_or(1);
        self.selected_request = Some(index);
        let char_index = line_start_char(&self.editor_text, line);
        self.jump_to_char(ctx, char_index);
    }

    /// 展开 / 折叠目录树中某个文件的请求列表（展开时懒解析）
    fn toggle_requests(&mut self, path: &Path) {
        let key = path.to_string_lossy().to_string();
        if self.expanded_files.remove(&key) {
            self.file_requests.remove(&key);
            return;
        }
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let requests = parser::parse(&text).to_output().requests;
                self.file_requests.insert(key.clone(), requests);
                self.expanded_files.insert(key);
            }
            Err(err) => self.error = Some(format!("读取失败: {err}")),
        }
    }

    /// 从目录树点击请求：打开对应文件并定位到该请求行
    fn open_request(&mut self, ctx: &egui::Context, path: PathBuf, index: usize) {
        // 多标签下允许直接切换，未保存内容保留在原标签中
        self.load_file(path);
        self.jump_to_request(ctx, index);
    }

    /// 从磁盘重新载入工作区请求历史
    fn refresh_history(&mut self) {
        self.history = match &self.active_root() {
            Some(root) => history::list(root),
            None => Vec::new(),
        };
    }

    fn clear_history(&mut self) {
        if let Some(root) = &self.active_root() {
            history::clear(root);
        }
        self.refresh_history();
        self.status = "已清空请求历史".to_owned();
    }

    /// 从工作区读取 client.global 变量快照
    fn refresh_globals(&mut self) {
        self.globals = match &self.active_root() {
            Some(root) => {
                let workspace = root.to_string_lossy().to_string();
                httpdeck_lib::get_globals_for(&self.state, &workspace).unwrap_or_default()
            }
            None => Vec::new(),
        };
    }

    fn clear_globals(&mut self) {
        let Some(root) = self.active_root() else {
            return;
        };
        let workspace = root.to_string_lossy().to_string();
        match httpdeck_lib::clear_globals_for(&self.state, &workspace) {
            Ok(()) => {
                self.refresh_globals();
                self.error = None;
                self.status = "已清空 client.global 变量".to_owned();
            }
            Err(err) => self.error = Some(err),
        }
    }

    /// 打开历史对应的文件并选中该请求；失败时返回 false
    fn open_history_entry(&mut self, entry: &HistoryEntry) -> bool {
        let Some(root) = self.active_root() else {
            return false;
        };
        let path = PathBuf::from(&entry.file);
        let path = if path.is_absolute() {
            path
        } else {
            root.join(path)
        };
        if !path.is_file() {
            self.error = Some(format!("历史文件不存在: {}", path.display()));
            return false;
        }
        self.load_file(path);
        let count = self
            .parsed
            .as_ref()
            .map_or(0, |parsed| parsed.requests.len());
        if entry.request_index >= count {
            self.error = Some("历史对应的请求已不存在".to_owned());
            return false;
        }
        self.selected_request = Some(entry.request_index);
        self.error = None;
        true
    }

    /// 重新发送历史中的请求
    fn rerun_history(&mut self, ctx: &egui::Context, entry: &HistoryEntry) {
        if self.open_history_entry(entry) {
            self.run_selected(ctx);
        }
    }

    /// 把编辑区光标移到指定字符位置，并滚动到可见区域
    fn jump_to_char(&mut self, ctx: &egui::Context, char_index: usize) {
        self.scroll_to_char = Some(char_index);
        let id = egui::Id::new("http_editor");
        if let Some(mut state) = egui::text_edit::TextEditState::load(ctx, id) {
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::one(
                    egui::text::CCursor::new(char_index),
                )));
            state.store(ctx, id);
        }
        ctx.memory_mut(|memory| memory.request_focus(id));
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
        for ws in &mut self.workspaces {
            ws.tree = workspace::scan_tree(&ws.root);
        }
    }

    fn set_pinned(&mut self, path: &Path, pinned: bool) {
        let Some(index) = self.workspace_index_for(path) else {
            return;
        };
        let root = self.workspaces[index].root.clone();
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
                self.workspaces[index].pinned = workspace::valid_pinned(&root, &config.pinned);
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

    fn handle_tree_action(&mut self, ctx: &egui::Context, action: tree::TreeAction) {
        let dialog = match action {
            tree::TreeAction::RunFolder(dir) => {
                self.run_folder(ctx, dir);
                return;
            }
            tree::TreeAction::ToggleRequests(path) => {
                self.toggle_requests(&path);
                return;
            }
            tree::TreeAction::OpenRequest(path, index) => {
                self.open_request(ctx, path, index);
                return;
            }
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
                self.update_tab_path(&path, Some(target.clone()));
                format!("已重命名为 {}", target.display())
            }),
            FileDialog::SaveAs { path, target } => {
                save_as_file(&path, Path::new(&target)).map(|()| format!("已另存为 {target}"))
            }
            FileDialog::Delete { path } => delete_file(&path).map(|()| {
                self.update_tab_path(&path, None);
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
                    let active_workspace = self.active_workspace_index();
                    if let Some(index) = active_workspace {
                        ui.weak(self.workspaces[index].root.display().to_string());
                    }
                    ui.separator();

                    ui.label("环境");
                    let selected_env = active_workspace
                        .and_then(|index| self.workspaces[index].selected_env.clone());
                    let envs = active_workspace
                        .map(|index| self.workspaces[index].envs.clone())
                        .unwrap_or_default();
                    let mut env_selection = None;
                    egui::ComboBox::from_id_salt("env")
                        .selected_text(selected_env.clone().unwrap_or_else(|| "无".to_owned()))
                        .show_ui(ui, |ui| {
                            if ui.selectable_label(selected_env.is_none(), "无").clicked() {
                                env_selection = Some(None);
                            }
                            for env in &envs {
                                let selected = selected_env.as_deref() == Some(env.as_str());
                                if ui.selectable_label(selected, env).clicked() {
                                    env_selection = Some(Some(env.clone()));
                                }
                            }
                        });
                    if let (Some(index), Some(selection)) = (active_workspace, env_selection) {
                        self.workspaces[index].selected_env = selection;
                    }
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
                        let label = if self.running > 1 {
                            format!("运行中 ×{}", self.running)
                        } else {
                            "运行中".to_owned()
                        };
                        ui.colored_label(ui.visuals().hyperlink_color, label)
                            .on_hover_text(self.status.as_str());
                    }
                    ui.separator();
                    if ui
                        .button("网络")
                        .on_hover_text("代理与 mTLS 客户端证书")
                        .clicked()
                    {
                        self.open_settings();
                    }
                    let mcp_running = self.mcp_status.as_ref().is_some_and(|s| s.running);
                    let label = if mcp_running { "MCP ●" } else { "MCP" };
                    if ui
                        .button(label)
                        .on_hover_text("MCP Server 状态与管理")
                        .clicked()
                    {
                        self.show_mcp = !self.show_mcp;
                        if self.show_mcp {
                            self.refresh_mcp_status();
                        }
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
                        self.refresh_tree();
                    }
                });
                ui.separator();

                let mut action = None;
                let mut open = None;
                let mut close_root = None;
                egui::ScrollArea::vertical()
                    .id_salt("tree_scroll")
                    .show(ui, |ui| {
                        for ws in &self.workspaces {
                            let state = tree::TreeState {
                                pinned: &ws.pinned,
                                selected: self.open_file.as_deref(),
                                expanded: &self.expanded_files,
                                file_requests: &self.file_requests,
                            };
                            let header = egui::CollapsingHeader::new(&ws.tree.name)
                                .id_salt(&ws.root)
                                .default_open(true)
                                .show(ui, |ui| {
                                    if !ws.pinned.is_empty() {
                                        ui.strong("置顶常用");
                                        for rel in &ws.pinned {
                                            let rel_path =
                                                rel.replace('/', std::path::MAIN_SEPARATOR_STR);
                                            let path = ws.root.join(&rel_path);
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
                                                &state,
                                                &mut open,
                                                &mut action,
                                            );
                                        }
                                        ui.separator();
                                    }
                                    for child in &ws.tree.children {
                                        tree::render(ui, child, &state, &mut open, &mut action);
                                    }
                                });
                            header
                                .header_response
                                .on_hover_text(ws.root.display().to_string())
                                .context_menu(|ui| {
                                    if ui.button("新建文件").clicked() {
                                        action = Some(tree::TreeAction::NewFile(ws.root.clone()));
                                        ui.close();
                                    }
                                    if ui.button("运行文件夹").clicked() {
                                        action = Some(tree::TreeAction::RunFolder(ws.root.clone()));
                                        ui.close();
                                    }
                                    ui.separator();
                                    if ui.button("关闭工作区").clicked() {
                                        close_root = Some(ws.root.clone());
                                        ui.close();
                                    }
                                });
                        }
                    });
                if let Some(path) = open {
                    self.load_file(path);
                }
                if let Some(action) = action {
                    self.handle_tree_action(&ui.ctx().clone(), action);
                }
                if let Some(root) = close_root {
                    self.close_workspace(&root);
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
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(self.requests_tab == RequestsTab::Requests, "请求")
                        .clicked()
                    {
                        self.requests_tab = RequestsTab::Requests;
                    }
                    if ui
                        .selectable_label(self.requests_tab == RequestsTab::History, "历史")
                        .clicked()
                    {
                        self.requests_tab = RequestsTab::History;
                        self.refresh_history();
                    }
                    if ui
                        .selectable_label(self.requests_tab == RequestsTab::Globals, "变量")
                        .on_hover_text("脚本写入的 client.global 变量")
                        .clicked()
                    {
                        self.requests_tab = RequestsTab::Globals;
                        self.refresh_globals();
                    }
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            let label = if self.floating_requests { "📌 取消" } else { "📌 置顶" };
                            let hover = if self.floating_requests {
                                "取消请求面板的浮动置顶模式"
                            } else {
                                "将请求面板钉到桌面，作为可拖动的小面板"
                            };
                            if ui.small_button(label).on_hover_text(hover).clicked() {
                                self.floating_requests = !self.floating_requests;
                            }
                        },
                    );
                });
                ui.separator();
                let ctx = ui.ctx().clone();
                match self.requests_tab {
                    RequestsTab::Requests => self.requests_list(ui, &ctx),
                    RequestsTab::History => self.history_list(ui, &ctx),
                    RequestsTab::Globals => self.globals_list(ui),
                }
            })
            .response
            .rect
    }

    /// 当前文件的请求列表
    /// 每行布局：▶ 运行按钮 + 请求名（可选中）+ 状态图标
    fn requests_list(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::ScrollArea::vertical()
            .id_salt("requests")
            .show(ui, |ui| {
                let mut clicked = None;
                let mut run = None;
                if let Some(parsed) = &self.parsed {
                    if parsed.requests.is_empty() {
                        ui.weak("当前文件没有可执行请求");
                    }
                    for (index, request) in parsed.requests.iter().enumerate() {
                        let running = self
                            .open_file
                            .as_deref()
                            .is_some_and(|file| self.request_is_running(file, index));
                        ui.horizontal(|ui| {
                            // ▶ 运行按钮：点一下直接执行该请求
                            let run_btn = ui
                                .small_button("▶")
                                .on_hover_text(format!(
                                    "运行 #{index} {} {}",
                                    request.method,
                                    request.display_name()
                                ));
                            if run_btn.clicked() {
                                run = Some(index);
                            }

                            // 请求名：点击选中并跳转
                            let label = format!("{} {}", request.method, request.display_name());
                            let resp = ui.add(
                                egui::Button::selectable(
                                    self.selected_request == Some(index),
                                    label,
                                )
                                .truncate(),
                            );
                            if resp.clicked() {
                                clicked = Some(index);
                            }
                            resp.context_menu(|ui| {
                                if ui.button("运行").clicked() {
                                    run = Some(index);
                                    ui.close();
                                }
                            });

                            // 状态图标
                            if running {
                                ui.spinner();
                            } else if let Some(result) = self.results.get(&index) {
                                let color = if result.error.is_some() {
                                    egui::Color32::from_rgb(230, 80, 80)
                                } else if result.status >= 500 {
                                    egui::Color32::from_rgb(230, 80, 80)
                                } else if result.status >= 400 {
                                    egui::Color32::from_rgb(230, 170, 70)
                                } else {
                                    egui::Color32::from_rgb(80, 200, 120)
                                };
                                let icon = if result.error.is_some() {
                                    "×"
                                } else if result.status >= 400 {
                                    "!"
                                } else {
                                    "✓"
                                };
                                ui.colored_label(color, icon);
                            }
                        });
                    }
                } else {
                    ui.weak("打开 .http 文件后显示请求列表");
                }
                if let Some(index) = clicked {
                    self.jump_to_request(ctx, index);
                }
                if let Some(index) = run {
                    self.selected_request = Some(index);
                    self.run_selected(ctx);
                }
            });
    }

    /// 历史列表：支持关键字过滤，点击「重发」重新执行
    fn history_list(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.history_search)
                    .id(egui::Id::new("history_search"))
                    .hint_text("搜索历史")
                    .desired_width((ui.available_width() - 44.0).max(40.0)),
            );
            if ui
                .small_button("清空")
                .on_hover_text("清空历史记录")
                .clicked()
            {
                self.clear_history();
            }
        });
        let keyword = self.history_search.clone();
        let items: Vec<HistoryEntry> = self
            .history
            .iter()
            .filter(|entry| history_matches(entry, &keyword))
            .cloned()
            .collect();
        egui::ScrollArea::vertical()
            .id_salt("history")
            .show(ui, |ui| {
                if items.is_empty() {
                    ui.weak("暂无历史记录");
                }
                for entry in &items {
                    let time = entry.time.get(11..19).unwrap_or(&entry.time);
                    let status = if entry.error.is_some() {
                        "ERR".to_owned()
                    } else {
                        entry.status.to_string()
                    };
                    let text = format!("{time}  {} {}  {}", entry.method, status, entry.name);
                    ui.horizontal(|ui| {
                        if ui
                            .small_button("重发")
                            .on_hover_text(format!("{} {}", entry.method, entry.url))
                            .clicked()
                        {
                            self.rerun_history(ctx, entry);
                        }
                        // 截断长名称，避免撑宽右侧面板
                        ui.add(
                            egui::Label::new(text)
                                .truncate()
                                .sense(egui::Sense::hover()),
                        )
                        .on_hover_text(format!("{}\n{}", entry.url, entry.time));
                    });
                }
            });
    }

    /// client.global 变量列表，支持刷新与一键清空
    fn globals_list(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.small_button("刷新").clicked() {
                self.refresh_globals();
            }
            if ui
                .small_button("清空")
                .on_hover_text("清空所有 client.global 变量")
                .clicked()
            {
                self.clear_globals();
            }
            ui.weak(format!("{} 项", self.globals.len()));
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .id_salt("globals")
            .show(ui, |ui| {
                if self.globals.is_empty() {
                    ui.weak("暂无 client.global 变量");
                }
                egui::Grid::new("globals_grid")
                    .striped(true)
                    .num_columns(2)
                    .show(ui, |ui| {
                        ui.strong("名称");
                        ui.strong("值");
                        ui.end_row();
                        for item in &self.globals {
                            ui.label(&item.name);
                            ui.add(
                                egui::Label::new(item.value.clone())
                                    .truncate()
                                    .sense(egui::Sense::hover()),
                            )
                            .on_hover_text(&item.value);
                            ui.end_row();
                        }
                    });
            });
    }

    fn response_panel(&mut self, root: &mut egui::Ui) -> egui::Rect {
        egui::Panel::bottom("response")
            .resizable(true)
            .default_size(self.response_height)
            .show(root, |ui| {
                ui.take_available_space();
                let result = self
                    .selected_request
                    .and_then(|index| self.results.get(&index));
                let mut all: Vec<(usize, &RequestResult)> = self
                    .results
                    .iter()
                    .map(|(index, item)| (*index, item))
                    .collect();
                all.sort_by_key(|(index, _)| *index);
                let file_key = self
                    .open_file
                    .as_ref()
                    .map(|file| file.to_string_lossy().to_string());
                let running: HashSet<usize> = file_key
                    .as_ref()
                    .map(|key| {
                        self.running_requests
                            .iter()
                            .filter(|(path, _)| path == key)
                            .map(|(_, index)| *index)
                            .collect()
                    })
                    .unwrap_or_default();
                let file_running = file_key
                    .as_ref()
                    .is_some_and(|key| self.running_files.contains(key));
                let clicked = response::ui(
                    ui,
                    result,
                    self.error.as_deref(),
                    &all,
                    self.selected_request,
                    &running,
                    file_running,
                );
                if let Some(index) = clicked {
                    self.selected_request = Some(index);
                }
            })
            .response
            .rect
    }

    fn editor_panel(&mut self, root: &mut egui::Ui) -> egui::Rect {
        egui::CentralPanel::default()
            .show(root, |ui| {
                if self.open_file.is_none() {
                    ui.centered_and_justified(|ui| {
                        ui.weak("从左侧目录打开一个 .http 文件");
                    });
                    return;
                }

                // 多标签栏：点击切换，● 标记未保存，✖ 关闭标签
                let mut switch_tab = None;
                let mut close_tab = None;
                if !self.tabs.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        for (index, tab) in self.tabs.iter().enumerate() {
                            let is_active = self.active_tab == Some(index);
                            let dirty = if is_active { self.dirty } else { tab.dirty };
                            let name = tab
                                .path
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or("未命名");
                            let label = if dirty {
                                format!("{name} ●")
                            } else {
                                name.to_owned()
                            };
                            if ui
                                .selectable_label(is_active, label)
                                .on_hover_text(tab.path.display().to_string())
                                .clicked()
                            {
                                switch_tab = Some(index);
                            }
                            if ui.small_button("✖").on_hover_text("关闭标签").clicked() {
                                close_tab = Some(index);
                            }
                        }
                    });
                    ui.separator();
                }
                if let Some(index) = switch_tab {
                    self.activate_tab(index);
                }
                if let Some(index) = close_tab {
                    self.activate_tab(index);
                    self.close_file();
                }
                let Some(path) = self.open_file.clone() else {
                    return;
                };

                let mut close_clicked = false;
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
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("✖").on_hover_text("关闭文件").clicked() {
                            close_clicked = true;
                        }
                    });
                });
                if close_clicked {
                    self.close_file();
                    return;
                }
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
                let gutter_w = (line_count.to_string().len() as f32 * digit_w + 30.0).ceil();
                // 行号栏与正文之间留出间隙，否则行首光标会被后绘制的行号栏覆盖；
                // 额外宽度用于绘制请求行的运行按钮
                let text_left = gutter_w as i8 + 8;

                let mut paint_info = None;
                let mut cursor_char = None;
                let mut ctrl_click_char = None;
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
                                left: text_left,
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
                        cursor_char = out.cursor_range.map(|range| range.primary.index.0);
                        if out.response.clicked() && ui.input(|input| input.modifiers.ctrl) {
                            ctrl_click_char = cursor_char;
                        }
                        // 查找 / 请求跳转是在 widget 运行前改的状态，TextEdit 检测不到选区变化，
                        // 不会自动滚动，这里手动把目标行滚动到可视区域中部
                        if let Some(char_index) = self.scroll_to_char.take() {
                            let cursor_rect = out
                                .galley
                                .pos_from_cursor(egui::text::CCursor::new(char_index))
                                .translate(out.galley_pos.to_vec2());
                            ui.scroll_to_rect(cursor_rect, Some(egui::Align::Center));
                        }
                        paint_info = Some((out.galley.clone(), out.galley_pos));
                    });

                let mut run_request = None;
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
                    // 请求所在行：行号左侧绘制运行按钮，点击直接执行该请求
                    if let Some(parsed) = &self.parsed {
                        for (index, request) in parsed.requests.iter().enumerate() {
                            let Some(row) = galley.rows.get(request.line.saturating_sub(1)) else {
                                continue;
                            };
                            let height = row.rect().height().max(font_id.size + 4.0);
                            let y = galley_pos.y + row.rect().min.y;
                            if y + height < viewport.top() || y > viewport.bottom() {
                                continue;
                            }
                            let rect = egui::Rect::from_min_size(
                                egui::pos2(viewport.left() + 1.0, y),
                                egui::vec2(15.0, height),
                            );
                            let response = ui
                                .interact(
                                    rect,
                                    egui::Id::new(("run_request", index)),
                                    egui::Sense::click(),
                                )
                                .on_hover_text(format!("运行 {}", request.display_name()));
                            let hovered = response.hovered();
                            // 悬停时高亮背景并提高图标对比度，明确可点击
                            if hovered {
                                painter.rect_filled(
                                    rect,
                                    3.0,
                                    ui.visuals().widgets.hovered.bg_fill,
                                );
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            } else if self.selected_request == Some(index) {
                                painter.rect_filled(rect, 3.0, ui.visuals().faint_bg_color);
                            }
                            let color = if hovered {
                                ui.visuals().widgets.hovered.fg_stroke.color
                            } else if self.selected_request == Some(index) {
                                ui.visuals().strong_text_color()
                            } else {
                                ui.visuals().weak_text_color()
                            };
                            painter.text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "▶",
                                font_id.clone(),
                                color,
                            );
                            if response.clicked() {
                                run_request = Some(index);
                            }
                        }
                    }
                }
                if let Some(index) = run_request {
                    let ctx = ui.ctx().clone();
                    self.selected_request = Some(index);
                    self.run_selected(&ctx);
                }

                let ctx = ui.ctx().clone();
                // Ctrl + 点击 {{变量}} 跳转到 @变量 定义行
                if let Some(cursor) = ctrl_click_char {
                    self.jump_to_variable(&ctx, cursor);
                }
                // 输入 {{ 前缀时展示变量补全候选
                if let Some((start, prefix)) =
                    cursor_char.and_then(|cursor| completion_context(&self.editor_text, cursor))
                {
                    let candidates = self.completion_candidates(&prefix);
                    if !candidates.is_empty() {
                        let mut chosen = None;
                        ui.horizontal_wrapped(|ui| {
                            ui.weak("补全变量：");
                            for name in candidates.iter().take(8) {
                                if ui.small_button(name).clicked() {
                                    chosen = Some(name.clone());
                                }
                            }
                        });
                        if let Some(name) = chosen {
                            let end = start + prefix.chars().count();
                            let cursor = self.insert_completion(start, end, &name);
                            self.jump_to_char(&ctx, cursor);
                        }
                    }
                }
            })
            .response
            .rect
    }

    /// 变量补全候选：当前文件的 @变量 与 client.global 名称
    fn completion_candidates(&self, prefix: &str) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        if let Some(parsed) = &self.parsed {
            names.extend(parsed.variables.iter().map(|(name, _)| name.clone()));
        }
        names.extend(self.globals.iter().map(|item| item.name.clone()));
        names.sort();
        names.dedup();
        names.retain(|name| name.starts_with(prefix) && name != prefix);
        names
    }

    /// 把 [start, end) 的补全前缀替换为变量名并补全 `}}`，返回新光标位置
    fn insert_completion(&mut self, start: usize, end: usize, name: &str) -> usize {
        let chars: Vec<char> = self.editor_text.chars().collect();
        let start = start.min(chars.len());
        let end = end.min(chars.len());
        let rest: String = chars[end..].iter().collect();
        let suffix = if rest.starts_with("}}") { "" } else { "}}" };
        let mut next: String = chars[..start].iter().collect();
        next.push_str(name);
        next.push_str(suffix);
        next.extend(chars[end..].iter());
        self.editor_text = next;
        self.dirty = true;
        self.reparse();
        start + name.chars().count() + suffix.chars().count()
    }

    /// 跳转到当前文件中 `@name` 的定义行
    fn jump_to_variable(&mut self, ctx: &egui::Context, cursor: usize) {
        let Some(name) = variable_at(&self.editor_text, cursor) else {
            self.error = Some("光标不在 {{变量}} 上".to_owned());
            return;
        };
        match variable_definition_line(&self.editor_text, &name) {
            Some(line) => {
                let index = line_start_char(&self.editor_text, line);
                self.jump_to_char(ctx, index);
                self.error = None;
                self.status = format!("跳转到变量 @{name}");
            }
            None => self.error = Some(format!("未在当前文件找到 @{name} 的定义")),
        }
    }

    /// 绘制整个界面，返回各区域实际矩形（便于测试布局）
    pub fn draw(&mut self, ui: &mut egui::Ui) -> LayoutRects {
        let ctx = ui.ctx().clone();
        self.poll_tasks(&ctx);
        self.handle_shortcuts(&ctx);
        self.sync_panels();
        let rects = LayoutRects {
            toolbar: self.toolbar(ui),
            tree: self.tree_panel(ui),
            requests: self.requests_panel(ui),
            response: self.response_panel(ui),
            central: self.editor_panel(ui),
        };
        self.track_response_height(&ctx, rects.response.height());
        self.render_file_dialog(&ctx);
        self.mcp_window(&ctx);
        self.settings_window(&ctx);
        self.floating_requests_window(&ctx);
        rects
    }

    /// 浮动请求面板：主窗口内的可拖动小窗口，显示精简或完整的请求列表
    fn floating_requests_window(&mut self, ctx: &egui::Context) {
        if !self.floating_requests {
            return;
        }
        let mut close_requested = false;
        let mut switch_mode = None;
        egui::Window::new("请求 📌")
            .id(egui::Id::new("floating_requests"))
            .resizable(true)
            .collapsible(false)
            .default_open(true)
            .default_pos(egui::pos2(800.0, 200.0))
            .default_size(if self.floating_full_mode {
                [360.0, 520.0]
            } else {
                [240.0, 360.0]
            })
            .min_size([180.0, 200.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.weak(
                        self.open_file
                            .as_deref()
                            .and_then(|path| path.file_name())
                            .and_then(|name| name.to_str())
                            .unwrap_or("未打开文件"),
                    );
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            let full = self.floating_full_mode;
                            let label = if full { "精简" } else { "完整" };
                            if ui.small_button(label).on_hover_text("切换视图模式").clicked() {
                                switch_mode = Some(!full);
                            }
                            if ui
                                .small_button("×")
                                .on_hover_text("取消浮动面板")
                                .clicked()
                            {
                                close_requested = true;
                            }
                        },
                    );
                });
                ui.separator();
                let ctx_clone = ctx.clone();
                if self.floating_full_mode {
                    egui::ScrollArea::vertical()
                        .id_salt("floating_requests_full")
                        .show(ui, |ui| {
                            self.requests_list(ui, &ctx_clone);
                        });
                } else {
                    self.floating_minimal_list(ui, &ctx_clone);
                }
            });
        if close_requested {
            self.floating_requests = false;
        }
        if let Some(next) = switch_mode {
            self.floating_full_mode = next;
        }
    }

    /// 精简模式的浮动请求列表：每行紧凑显示 ▶ 请求名 状态
    fn floating_minimal_list(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::ScrollArea::vertical()
            .id_salt("floating_requests_minimal")
            .show(ui, |ui| {
                let mut clicked = None;
                let mut run = None;
                if let Some(parsed) = &self.parsed {
                    if parsed.requests.is_empty() {
                        ui.weak("无请求");
                    }
                    for (index, request) in parsed.requests.iter().enumerate() {
                        let running = self
                            .open_file
                            .as_deref()
                            .is_some_and(|file| self.request_is_running(file, index));
                        ui.horizontal(|ui| {
                            // ▶ 运行按钮
                            if ui.small_button("▶").clicked() {
                                run = Some(index);
                            }

                            // 请求名
                            let name = request.display_name();
                            let label = if name.is_empty() {
                                format!("{} #{}", request.method, index + 1)
                            } else {
                                name.to_owned()
                            };
                            if ui.add(
                                egui::Button::selectable(
                                    self.selected_request == Some(index),
                                    label,
                                )
                                .truncate(),
                            ).clicked() {
                                clicked = Some(index);
                            }

                            // 状态图标
                            if running {
                                ui.spinner();
                            } else if let Some(result) = self.results.get(&index) {
                                let color = if result.error.is_some() {
                                    egui::Color32::from_rgb(230, 80, 80)
                                } else if result.status >= 400 {
                                    egui::Color32::from_rgb(230, 170, 70)
                                } else {
                                    egui::Color32::from_rgb(80, 200, 120)
                                };
                                let icon = if result.error.is_some() {
                                    "×"
                                } else if result.status >= 400 {
                                    "!"
                                } else {
                                    "✓"
                                };
                                ui.colored_label(color, icon);
                            }
                        });
                    }
                } else {
                    ui.weak("打开文件后显示");
                }
                if let Some(index) = clicked {
                    self.jump_to_request(ctx, index);
                }
                if let Some(index) = run {
                    self.selected_request = Some(index);
                    self.run_selected(ctx);
                }
            });
    }

    /// 记录响应面板高度；拖拽结束后写入界面状态文件
    fn track_response_height(&mut self, ctx: &egui::Context, height: f32) {
        if (height - self.response_height).abs() >= 1.0 {
            self.response_height = height;
            self.ui_state_dirty = true;
        }
        let pointer_down = ctx.input(|input| input.pointer.any_down());
        if self.ui_state_dirty && !pointer_down {
            self.ui_state_dirty = false;
            self.ui_state.response_height = Some(self.response_height);
            save_ui_state_to(&ui_state_path(), &self.ui_state);
        }
    }

    fn open_settings(&mut self) {
        let config = self
            .active_root()
            .map(|root| workspace::load_config(&root))
            .unwrap_or_default();
        self.settings_proxy = config.proxy.unwrap_or_default();
        self.settings_cert = config.client_cert.unwrap_or_default();
        self.settings_key = config.client_key.unwrap_or_default();
        self.show_settings = true;
    }

    fn save_settings(&mut self) {
        let Some(root) = self.active_root() else {
            return;
        };
        if let Some(message) = proxy_problem(&self.settings_proxy) {
            self.error = Some(message);
            return;
        }
        let optional = |value: &str| {
            let value = value.trim();
            (!value.is_empty()).then(|| value.to_owned())
        };
        let mut config = workspace::load_config(&root);
        config.proxy = optional(&self.settings_proxy);
        config.client_cert = optional(&self.settings_cert);
        config.client_key = optional(&self.settings_key);
        match workspace::save_config(&root, &config) {
            Ok(()) => {
                self.show_settings = false;
                self.error = None;
                self.status = "网络设置已保存".to_owned();
            }
            Err(err) => self.error = Some(err),
        }
    }

    /// 网络设置窗口：代理与 mTLS 客户端证书
    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = self.show_settings;
        egui::Window::new("网络设置")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label("代理地址");
                ui.add(
                    egui::TextEdit::singleline(&mut self.settings_proxy)
                        .id(egui::Id::new("settings_proxy"))
                        .hint_text("http://127.0.0.1:7890 或 socks5://127.0.0.1:1080")
                        .desired_width(360.0),
                );
                ui.weak("留空表示直连，支持 HTTP / SOCKS5 代理");
                ui.separator();

                ui.label("mTLS 客户端证书（PEM）");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.settings_cert)
                            .id(egui::Id::new("settings_cert"))
                            .desired_width(280.0),
                    );
                    if ui.button("选择…").clicked() {
                        if let Some(file) = rfd::FileDialog::new()
                            .add_filter("PEM", &["pem", "crt", "cer"])
                            .pick_file()
                        {
                            self.settings_cert = file.to_string_lossy().to_string();
                        }
                    }
                });
                ui.label("客户端私钥（PEM，可选）");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.settings_key)
                            .id(egui::Id::new("settings_key"))
                            .desired_width(280.0),
                    );
                    if ui.button("选择…").clicked() {
                        if let Some(file) = rfd::FileDialog::new()
                            .add_filter("PEM", &["pem", "key"])
                            .pick_file()
                        {
                            self.settings_key = file.to_string_lossy().to_string();
                        }
                    }
                });
                ui.weak("留空表示不使用客户端证书；相对路径按工作区根目录解析");
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("保存").clicked() {
                        self.save_settings();
                    }
                    if ui.button("取消").clicked() {
                        self.show_settings = false;
                    }
                });
            });
        self.show_settings = open;
    }

    fn refresh_mcp_status(&mut self) {
        self.mcp_status = Some(httpdeck_lib::mcp_status());
    }

    /// 启动 MCP Server；阻塞操作放到后台线程，避免卡住界面
    fn start_mcp(&mut self, ctx: &egui::Context) {
        if self.mcp_busy {
            return;
        }
        self.mcp_busy = true;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = start_mcp_process();
            let _ = tx.send(TaskMsg::McpDone(result));
            ctx.request_repaint();
        });
    }

    fn stop_mcp(&mut self, ctx: &egui::Context) {
        if self.mcp_busy {
            return;
        }
        self.mcp_busy = true;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = stop_mcp_process();
            let _ = tx.send(TaskMsg::McpDone(result));
            ctx.request_repaint();
        });
    }

    /// MCP 状态窗口：运行状态、PID/端口/地址、启动与停止
    fn mcp_window(&mut self, ctx: &egui::Context) {
        if !self.show_mcp {
            return;
        }
        let mut open = self.show_mcp;
        egui::Window::new("MCP Server")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                match &self.mcp_status {
                    None => {
                        ui.weak("未获取状态");
                    }
                    Some(status) if status.running => {
                        ui.colored_label(egui::Color32::from_rgb(90, 190, 120), "运行中");
                        if let Some(pid) = status.pid {
                            ui.label(format!("PID {pid}"));
                        }
                        if let Some(port) = status.port {
                            ui.label(format!("端口 {port}"));
                        }
                        if let Some(endpoint) = &status.endpoint {
                            ui.label(format!("地址 {endpoint}"));
                        }
                        if let Some(started) = &status.started_at {
                            ui.weak(format!("启动于 {started}"));
                        }
                    }
                    Some(_) => {
                        ui.weak("未运行");
                    }
                }
                ui.separator();
                if let Some(status) = &self.mcp_status {
                    ui.weak(match &status.exe_path {
                        Some(path) => format!("可执行文件 {path}"),
                        None => "未找到 httpdeck-mcp 可执行文件".to_owned(),
                    });
                    ui.weak(format!("状态文件 {}", status.lock_path));
                }
                ui.separator();
                ui.horizontal(|ui| {
                    let running = self.mcp_status.as_ref().is_some_and(|s| s.running);
                    let ctx = ui.ctx().clone();
                    if ui
                        .add_enabled(!running && !self.mcp_busy, egui::Button::new("启动"))
                        .clicked()
                    {
                        self.start_mcp(&ctx);
                    }
                    if ui
                        .add_enabled(running && !self.mcp_busy, egui::Button::new("停止"))
                        .clicked()
                    {
                        self.stop_mcp(&ctx);
                    }
                    if ui
                        .add_enabled(!self.mcp_busy, egui::Button::new("刷新"))
                        .clicked()
                    {
                        self.refresh_mcp_status();
                    }
                    if self.mcp_busy {
                        ui.spinner();
                    }
                });
            });
        self.show_mcp = open;
    }
}

/// 启动内置 MCP Server（在后台线程调用）
fn start_mcp_process() -> Result<McpStatus, String> {
    if httpdeck_lib::mcp_lock::read_info().is_some() {
        return Ok(httpdeck_lib::mcp_status());
    }
    let exe = httpdeck_lib::mcp_lock::mcp_exe_path()
        .ok_or_else(|| "未找到 httpdeck-mcp 可执行文件，请先构建".to_string())?;
    let port = 18787u16;
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("--port")
        .arg(port.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("启动 httpdeck-mcp 失败: {e}"))?;
    std::thread::sleep(std::time::Duration::from_millis(500));
    match httpdeck_lib::mcp_lock::read_info() {
        Some(_) => Ok(httpdeck_lib::mcp_status()),
        None => {
            let _ = child.wait();
            Err("httpdeck-mcp 启动后未写入状态文件（可能端口被占用或启动失败）".to_owned())
        }
    }
}

/// 停止内置 MCP Server（在后台线程调用）
fn stop_mcp_process() -> Result<McpStatus, String> {
    let info =
        httpdeck_lib::mcp_lock::read_info().ok_or_else(|| "MCP server 未运行".to_string())?;
    httpdeck_lib::mcp_lock::kill_by_pid(info.pid)?;
    Ok(httpdeck_lib::mcp_status())
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

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        self.ui_state.response_height = Some(self.response_height);
        save_ui_state_to(&ui_state_path(), &self.ui_state);
    }
}

/// 界面状态文件位置：Windows 用 %APPDATA%，其余平台用 $HOME
fn ui_state_path() -> PathBuf {
    // 测试不读写真实用户配置，避免污染
    #[cfg(test)]
    {
        return std::env::temp_dir().join("httpdeck-ui-test-state.json");
    }
    #[cfg(not(test))]
    if let Ok(appdata) = std::env::var("APPDATA") {
        return PathBuf::from(appdata).join("HttpDeck").join("ui.json");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".httpdeck").join("ui.json");
    }
    std::env::temp_dir().join("httpdeck-ui.json")
}

fn load_ui_state_from(path: &Path) -> UiState {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_ui_state_to(path: &Path, state: &UiState) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(state) {
        let _ = std::fs::write(path, text);
    }
}

/// 检查代理地址格式；空表示直连，返回错误文案
fn proxy_problem(proxy: &str) -> Option<String> {
    let proxy = proxy.trim();
    if proxy.is_empty() {
        return None;
    }
    let lower = proxy.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("socks5://")
    {
        None
    } else {
        Some("代理地址应以 http://、https:// 或 socks5:// 开头".to_owned())
    }
}

/// 历史记录是否匹配搜索关键字（方法 / URL / 名称 / 文件 / 状态码）
fn history_matches(entry: &HistoryEntry, keyword: &str) -> bool {
    let keyword = keyword.trim().to_lowercase();
    if keyword.is_empty() {
        return true;
    }
    entry.method.to_lowercase().contains(&keyword)
        || entry.url.to_lowercase().contains(&keyword)
        || entry.name.to_lowercase().contains(&keyword)
        || entry.file.to_lowercase().contains(&keyword)
        || entry.status.to_string().contains(&keyword)
}

/// 目录批量运行完成后的状态栏文案
fn folder_run_summary(file_count: usize, request_count: usize, failed: usize) -> String {
    if failed > 0 {
        format!("已运行 {file_count} 个文件（{failed} 个失败），共 {request_count} 个请求")
    } else {
        format!("已运行 {file_count} 个文件，共 {request_count} 个请求")
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

/// 1-based 行号 → 该行行首的字符索引（越界时返回文本末尾）
fn line_start_char(text: &str, line: usize) -> usize {
    if line <= 1 {
        return 0;
    }
    let mut chars = 0;
    for (index, row) in text.split_inclusive('\n').enumerate() {
        if index + 1 == line {
            return chars;
        }
        chars += row.chars().count();
    }
    chars
}

/// 光标是否处在未完成的 `{{prefix` 中，返回（前缀起始字符索引，前缀）
fn completion_context(text: &str, cursor: usize) -> Option<(usize, String)> {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    let mut start = None;
    let mut index = cursor;
    while index > 0 {
        index -= 1;
        match chars[index] {
            '\n' | '}' => return None,
            '{' => {
                if index > 0 && chars[index - 1] == '{' {
                    start = Some(index + 1);
                }
                break;
            }
            _ => {}
        }
    }
    let start = start?;
    let prefix: String = chars[start..cursor].iter().collect();
    if prefix.contains(['{', '}', ' ', '\t']) {
        return None;
    }
    Some((start, prefix))
}

/// 找到光标所在的 `{{变量}}` 名称（忽略过滤器，如 `{{host | default}}`）
fn variable_at(text: &str, cursor: usize) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    let line_start = chars[..cursor]
        .iter()
        .rposition(|ch| *ch == '\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let line_end = chars[cursor..]
        .iter()
        .position(|ch| *ch == '\n')
        .map(|offset| cursor + offset)
        .unwrap_or(chars.len());
    let line = &chars[line_start..line_end];
    let offset = cursor - line_start;
    let mut index = 0;
    while index + 1 < line.len() {
        if line[index] == '{' && line[index + 1] == '{' {
            if let Some(rel_end) = line[index + 2..].iter().position(|ch| *ch == '}') {
                let close = index + 2 + rel_end;
                if line.get(close + 1) == Some(&'}') {
                    if offset >= index && offset <= close + 2 {
                        let name: String = line[index + 2..close].iter().collect();
                        let name = name.split('|').next().unwrap_or("").trim();
                        if !name.is_empty() {
                            return Some(name.to_owned());
                        }
                    }
                    index = close + 2;
                    continue;
                }
            }
        }
        index += 1;
    }
    None
}

/// 变量 `@name = ...` 定义所在行号（1 起）
fn variable_definition_line(text: &str, name: &str) -> Option<usize> {
    for (index, line) in text.lines().enumerate() {
        let Some(rest) = line.trim_start().strip_prefix('@') else {
            continue;
        };
        let Some((key, _)) = rest.split_once('=') else {
            continue;
        };
        if key.trim() == name {
            return Some(index + 1);
        }
    }
    None
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

    /// 打开文件后布局仍需保持三栏 + 响应面板，且编辑器左栏渲染请求运行按钮
    #[test]
    fn layout_stable_with_file_open() {
        let dir = std::env::temp_dir().join(format!("httpdeck-egui-layout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("api")).unwrap();
        let file = dir.join("api").join("demo.http");
        std::fs::write(
            &file,
            "@host = http://127.0.0.1:1\n\n### 本地 JSON\nGET {{host}}/json\n\n### 底部请求\nPOST {{host}}/post\nContent-Type: application/json\n\n{\"name\":\"x\"}\n",
        )
        .unwrap();

        let mut app = HttpDeckApp::new();
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(1.5);
        app.load_workspace(dir.clone());
        app.load_file(file.clone());

        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 820.0),
            )),
            ..Default::default()
        };
        let mut texts = Vec::new();
        // 多画若干帧，防止面板宽度逐帧累积膨胀（曾因行内按钮预留宽度不足导致）
        for _ in 0..30 {
            let mut rects = None;
            let mut output = ctx.run_ui(input.clone(), |ui| {
                rects = Some(app.draw(ui));
            });
            output.textures_delta.clear();
            texts = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
            let rects = rects.expect("draw 未执行");
            assert!(
                rects.tree.width() < 600.0,
                "目录面板不应撑满窗口: {:?}",
                rects.tree
            );
            assert!(
                rects.requests.width() > 100.0,
                "请求面板应保持可见: {:?}",
                rects.requests
            );
            assert!(
                rects.response.height() > 100.0,
                "响应面板应保持可见: {:?}",
                rects.response
            );
        }
        // 编辑区行首有 2 个 ▶，请求面板每行也有 1 个 ▶ small_button，共 4 个
        assert_eq!(
            texts.iter().filter(|text| text.as_str() == "▶").count(),
            4,
            "每个请求行应渲染一个运行按钮，实际: {texts:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 后打开的同名工作区不能覆盖先打开的工作区，请求也应使用各自的工作区配置
    #[test]
    fn same_named_workspaces_stay_visible_with_independent_run_contexts() {
        let base = std::env::temp_dir().join(format!(
            "httpdeck-egui-multi-workspace-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let root_a = base.join("a").join("api");
        let root_b = base.join("b").join("api");
        std::fs::create_dir_all(&root_a).unwrap();
        std::fs::create_dir_all(&root_b).unwrap();
        let file_a = root_a.join("a.http");
        let file_b = root_b.join("b.http");
        std::fs::write(&file_a, "GET https://a.example.com\n").unwrap();
        std::fs::write(&file_b, "GET https://b.example.com\n").unwrap();

        let mut app = HttpDeckApp::new();
        app.workspaces.clear();
        app.load_workspace(root_a.clone());
        app.load_workspace(root_b.clone());
        assert_eq!(app.workspaces.len(), 2, "两个同名工作区应同时保留");

        app.load_file(file_a.clone());
        assert_eq!(
            app.run_context(&file_a).map(|(root, _)| root),
            Some(root_a.clone())
        );
        app.load_file(file_b.clone());
        assert_eq!(
            app.run_context(&file_b).map(|(root, _)| root),
            Some(root_b.clone())
        );

        let ctx = egui::Context::default();
        let mut texts = Vec::new();
        for _ in 0..3 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 820.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    app.draw(ui);
                },
            );
            output.textures_delta.clear();
            texts = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
        }
        assert_eq!(
            texts.iter().filter(|text| text.as_str() == "api").count(),
            2,
            "目录树应同时显示两个 api 根目录: {texts:?}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// 鼠标悬停在行首运行按钮上时应切换为手型光标（并高亮）
    #[test]
    fn run_marker_hover_shows_hand_cursor() {
        let (dir, a, _b) = tab_workspace("hover-run");
        let mut app = HttpDeckApp::new();
        app.load_workspace(dir.clone());
        app.load_file(a.clone());

        let ctx = egui::Context::default();
        let mut draw = |events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 820.0),
                )),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                app.draw(ui);
            });
            output.textures_delta.clear();
            // 取最后一个 "▶"——那是编辑区行首的自定义运行按钮（请求面板的 small_button 在前）
            let marker = output
                .shapes
                .iter()
                .rev()
                .find_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) if text.galley.text() == "▶" => Some(text.pos),
                    _ => None,
                });
            (output, marker)
        };
        let (_, marker) = draw(Vec::new());
        let pos = marker.expect("应渲染行首运行按钮");
        let (output, _) = draw(vec![egui::Event::PointerMoved(pos + egui::vec2(4.0, 4.0))]);
        assert_eq!(
            output.platform_output.cursor_icon,
            egui::CursorIcon::PointingHand,
            "悬停运行按钮应显示手型光标"
        );

        let _ = std::fs::remove_dir_all(&dir);
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

    /// 目录树点击请求：打开文件并把光标定位到该请求所在行
    #[test]
    fn open_request_jumps_to_request_line() {
        let dir = std::env::temp_dir().join(format!("httpdeck-egui-jump-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("demo.http");
        let text = "@host = http://127.0.0.1:1\n\n### 本地 JSON\nGET {{host}}/json\n\n### 底部请求\nPOST {{host}}/post\n";
        std::fs::write(&file, text).unwrap();

        let mut app = HttpDeckApp::new();
        let ctx = egui::Context::default();
        app.load_workspace(dir.clone());
        app.open_request(&ctx, file.clone(), 1);

        assert_eq!(app.open_file.as_deref(), Some(file.as_path()));
        assert_eq!(app.selected_request, Some(1));
        assert_eq!(app.scroll_to_char, Some(line_start_char(text, 7)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 展开目录树中的文件时应懒解析出请求列表，再次点击折叠并清理缓存
    #[test]
    fn toggle_requests_parses_file() {
        let dir = std::env::temp_dir().join(format!("httpdeck-egui-expand-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("demo.http");
        std::fs::write(
            &file,
            "### 登录\nPOST https://example.com/login\nContent-Type: application/json\n\n{\"user\":\"a\"}\n",
        )
        .unwrap();
        let mut app = HttpDeckApp::new();
        app.toggle_requests(&file);
        let key = file.to_string_lossy().to_string();
        assert!(app.expanded_files.contains(&key));
        assert_eq!(app.file_requests.get(&key).map(Vec::len), Some(1));
        app.toggle_requests(&file);
        assert!(!app.expanded_files.contains(&key));
        assert!(app.file_requests.get(&key).is_none());
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
        app.workspaces.clear();
        app.load_workspace(dir.clone());
        let ctx = egui::Context::default();

        app.handle_tree_action(&ctx, tree::TreeAction::SetPinned(file.clone(), true));
        assert_eq!(app.workspaces[0].pinned, vec!["demo.http"]);
        assert_eq!(workspace::load_config(&dir).pinned, vec!["demo.http"]);

        app.handle_tree_action(&ctx, tree::TreeAction::SetPinned(file, false));
        assert!(app.workspaces[0].pinned.is_empty());
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

    /// 关闭文件应清空编辑区状态；有未保存修改时拒绝关闭并提示
    #[test]
    fn close_file_clears_editor_state() {
        let mut app = HttpDeckApp::new();
        app.open_file = Some(PathBuf::from("test.http"));
        app.editor_text = "GET a\n".to_owned();
        app.parsed = Some(parser::parse(&app.editor_text));
        app.selected_request = Some(0);
        app.dirty = true;

        app.close_file();
        assert!(app.open_file.is_some(), "未保存时不应关闭文件");
        assert!(app.error.is_some(), "应提示未保存");

        app.dirty = false;
        app.close_file();
        assert!(app.open_file.is_none());
        assert!(app.editor_text.is_empty());
        assert!(app.parsed.is_none());
        assert!(app.selected_request.is_none());
        assert!(app.error.is_none());
    }

    /// 点击请求列表应把光标定位到该请求所在行
    #[test]
    fn request_click_jumps_to_request_line() {
        let mut app = HttpDeckApp::new();
        app.open_file = Some(PathBuf::from("test.http"));
        app.editor_text = "### one\nGET a\n\n### two\nPOST b\n".to_owned();
        app.parsed = Some(parser::parse(&app.editor_text));
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1280.0, 820.0),
            )),
            focused: true,
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            app.draw(ui);
        });
        output.textures_delta.clear();

        app.jump_to_request(&ctx, 1);
        let expected = line_start_char(&app.editor_text, 5);
        assert_eq!(app.selected_request, Some(1));
        assert_eq!(app.scroll_to_char, Some(expected));
        let state = egui::text_edit::TextEditState::load(&ctx, egui::Id::new("http_editor"))
            .expect("应存在编辑器状态");
        assert_eq!(
            state
                .cursor
                .char_range()
                .map(|range| range.primary.index.0 as usize),
            Some(expected)
        );
        assert!(ctx.memory(|memory| memory.has_focus(egui::Id::new("http_editor"))));
    }

    /// 行号到字符索引的换算
    #[test]
    fn line_start_char_handles_lines() {
        let text = "a\nbc\n\n中文\n";
        assert_eq!(line_start_char(text, 1), 0);
        assert_eq!(line_start_char(text, 2), 2);
        assert_eq!(line_start_char(text, 4), 6);
        assert_eq!(line_start_char(text, 99), text.chars().count());
    }

    fn tab_workspace(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("httpdeck-egui-tabs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.http");
        let b = dir.join("b.http");
        std::fs::write(&a, "GET https://example.com/a\n").unwrap();
        std::fs::write(&b, "GET https://example.com/b\n").unwrap();
        (dir, a, b)
    }

    /// 打开多个文件形成标签，切换时保留各自文本与未保存状态
    #[test]
    fn tabs_keep_edits_when_switching() {
        let (dir, a, b) = tab_workspace("switch");
        let mut app = HttpDeckApp::new();
        app.load_workspace(dir.clone());
        app.load_file(a.clone());
        app.editor_text.push_str("### 未保存\n");
        app.dirty = true;
        app.reparse();

        app.load_file(b.clone());
        assert_eq!(app.tabs.len(), 2, "打开第二个文件应新增标签");
        assert_eq!(app.open_file.as_deref(), Some(b.as_path()));
        assert!(!app.dirty, "新标签不应继承未保存状态");

        app.load_file(a.clone());
        assert_eq!(app.active_tab, Some(0));
        assert!(app.dirty, "切回后应恢复未保存标记");
        assert!(app.editor_text.contains("### 未保存"));

        // 已打开的文件再次点击只切换，不应重复创建标签
        app.load_file(a.clone());
        assert_eq!(app.tabs.len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 标签栏应显示所有打开文件，并用 ● 标记非当前标签的未保存修改
    #[test]
    fn tab_bar_marks_unsaved() {
        let (dir, a, b) = tab_workspace("bar");
        let mut app = HttpDeckApp::new();
        app.load_workspace(dir.clone());
        app.load_file(a.clone());
        app.load_file(b.clone());
        app.tabs[0].dirty = true;

        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 820.0),
            )),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            app.draw(ui);
        });
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
            texts.iter().any(|text| text == "a.http ●"),
            "未保存标签应带 ● 标记: {texts:?}"
        );
        assert!(
            texts.iter().any(|text| text == "b.http"),
            "当前标签应显示文件名: {texts:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 关闭当前标签后自动激活相邻标签
    #[test]
    fn closing_tab_activates_neighbor() {
        let (dir, a, b) = tab_workspace("close");
        let mut app = HttpDeckApp::new();
        app.load_workspace(dir.clone());
        app.load_file(a.clone());
        app.load_file(b.clone());
        assert_eq!(app.active_tab, Some(1));

        app.close_file();
        assert_eq!(app.tabs.len(), 1);
        assert_eq!(app.active_tab, Some(0));
        assert_eq!(app.open_file.as_deref(), Some(a.as_path()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 界面状态文件可序列化并恢复响应面板高度
    #[test]
    fn ui_state_round_trip() {
        let path =
            std::env::temp_dir().join(format!("httpdeck-ui-state-{}.json", std::process::id()));
        let state = UiState {
            response_height: Some(420.0),
        };
        save_ui_state_to(&path, &state);
        assert_eq!(load_ui_state_from(&path).response_height, Some(420.0));

        std::fs::remove_file(&path).unwrap();
        assert_eq!(load_ui_state_from(&path).response_height, None);
    }

    /// 响应面板首次布局应使用上次保存的高度
    #[test]
    fn response_panel_uses_saved_height() {
        let mut app = HttpDeckApp::new();
        app.response_height = 420.0;
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 820.0),
            )),
            ..Default::default()
        };
        let mut rects = None;
        let mut output = ctx.run_ui(input, |ui| {
            rects = Some(app.draw(ui));
        });
        output.textures_delta.clear();
        let height = rects.unwrap().response.height();
        assert!(
            (height - 420.0).abs() < 40.0,
            "响应面板高度应接近保存值，实际 {height}"
        );
    }

    /// 输入 `{{` 前缀时应识别补全上下文，闭合后不再补全
    #[test]
    fn completion_context_detects_prefix() {
        let text = "GET {{ho";
        assert_eq!(
            completion_context(text, text.chars().count()),
            Some((6, "ho".to_owned()))
        );
        let closed = "GET {{host}}";
        assert_eq!(completion_context(closed, closed.chars().count()), None);
        assert_eq!(completion_context("GET {", 6), None);
        assert_eq!(completion_context("{{ us", 5), None);
    }

    /// Ctrl+点击定位变量名，并找到 @变量 定义行
    #[test]
    fn variable_lookup_and_definition() {
        let text = "GET {{host}}/api";
        assert_eq!(variable_at(text, 7), Some("host".to_owned()));
        assert_eq!(
            variable_at("GET {{host | default('x')}}", 8),
            Some("host".to_owned())
        );
        assert_eq!(variable_at(text, 0), None);

        let file = "### 一\n@host = http://x\nGET {{host}}\n@token=abc\n";
        assert_eq!(variable_definition_line(file, "host"), Some(2));
        assert_eq!(variable_definition_line(file, "token"), Some(4));
        assert_eq!(variable_definition_line(file, "missing"), None);
    }

    /// 补全写入变量名与 `}}`，已有闭合时不重复添加
    #[test]
    fn completion_inserts_variable_and_marks_dirty() {
        let mut app = HttpDeckApp::new();
        app.editor_text = "GET {{ho".to_owned();
        app.reparse();
        let cursor = app.insert_completion(6, 8, "host");
        assert_eq!(app.editor_text, "GET {{host}}");
        assert!(app.dirty);
        assert_eq!(cursor, app.editor_text.chars().count());

        app.editor_text = "GET {{ho}}".to_owned();
        app.reparse();
        let cursor = app.insert_completion(6, 8, "host");
        assert_eq!(app.editor_text, "GET {{host}}");
        assert_eq!(cursor, 10);
    }

    /// 补全候选来自文件变量与 client.global，前缀匹配且排除已完整输入的名称
    #[test]
    fn completion_candidates_match_prefix() {
        let mut app = HttpDeckApp::new();
        app.editor_text = "@host = http://x\n@hostname = y\nGET {{ho".to_owned();
        app.reparse();
        assert_eq!(
            app.completion_candidates("ho"),
            vec!["host".to_owned(), "hostname".to_owned()]
        );
        assert_eq!(
            app.completion_candidates("host"),
            vec!["hostname".to_owned()]
        );

        app.globals = vec![KeyValue {
            name: "hostGlobal".to_owned(),
            value: "z".to_owned(),
        }];
        assert_eq!(
            app.completion_candidates("host"),
            vec!["hostGlobal".to_owned(), "hostname".to_owned()]
        );
    }

    /// 网络设置应校验代理地址并持久化代理与证书路径
    #[test]
    fn settings_save_persists_proxy_and_cert() {
        let (dir, _a, _b) = tab_workspace("settings");
        let mut app = HttpDeckApp::new();
        app.load_workspace(dir.clone());
        app.settings_proxy = " socks5://127.0.0.1:1080 ".to_owned();
        app.settings_cert = "certs/client.pem".to_owned();
        app.settings_key = "certs/client.key".to_owned();
        app.save_settings();
        assert!(app.error.is_none());

        let config = workspace::load_config(&dir);
        assert_eq!(config.proxy.as_deref(), Some("socks5://127.0.0.1:1080"));
        assert_eq!(config.client_cert.as_deref(), Some("certs/client.pem"));
        assert_eq!(config.client_key.as_deref(), Some("certs/client.key"));

        app.settings_proxy = "127.0.0.1:7890".to_owned();
        app.save_settings();
        assert!(app.error.is_some(), "缺少协议前缀应提示错误");
        assert_eq!(
            workspace::load_config(&dir).proxy.as_deref(),
            Some("socks5://127.0.0.1:1080"),
            "非法代理不应覆盖已保存配置"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 网络设置窗口应展示代理与 mTLS 输入项
    #[test]
    fn settings_window_renders_fields() {
        let mut app = HttpDeckApp::new();
        app.show_settings = true;
        let ctx = egui::Context::default();
        let mut texts = Vec::new();
        for _ in 0..2 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 820.0),
                )),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                app.draw(ui);
            });
            output.textures_delta.clear();
            texts = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
        }
        assert!(texts.iter().any(|text| text == "网络设置"), "{texts:?}");
        assert!(texts.iter().any(|text| text == "代理地址"), "{texts:?}");
        assert!(
            texts.iter().any(|text| text.contains("mTLS 客户端证书")),
            "{texts:?}"
        );
    }

    /// MCP 状态窗口应展示运行状态与操作按钮
    #[test]
    fn mcp_window_renders_status() {
        let mut app = HttpDeckApp::new();
        app.show_mcp = true;
        app.mcp_status = Some(McpStatus {
            running: false,
            pid: None,
            port: None,
            endpoint: None,
            started_at: None,
            lock_path: "C:/tmp/mcp-server.json".to_owned(),
            exe_path: None,
        });
        let ctx = egui::Context::default();
        let mut texts = Vec::new();
        // 窗口是独立的 Area，需要第二帧才会真正布局并产出文本图形
        for _ in 0..2 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 820.0),
                )),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                app.draw(ui);
            });
            output.textures_delta.clear();
            texts = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
        }
        assert!(texts.iter().any(|text| text == "MCP Server"), "{texts:?}");
        assert!(texts.iter().any(|text| text == "未运行"), "{texts:?}");
        assert!(texts.iter().any(|text| text == "启动"), "{texts:?}");
    }

    /// 目录批量运行状态栏文案应包含文件数、请求数与失败数
    #[test]
    fn folder_summary_mentions_failures() {
        assert_eq!(folder_run_summary(2, 5, 0), "已运行 2 个文件，共 5 个请求");
        assert_eq!(
            folder_run_summary(2, 5, 1),
            "已运行 2 个文件（1 个失败），共 5 个请求"
        );
    }

    fn write_history(dir: &Path, entries: &[HistoryEntry]) {
        let store = dir.join(".http-client").join("history.json");
        std::fs::create_dir_all(store.parent().unwrap()).unwrap();
        std::fs::write(&store, serde_json::to_string(entries).unwrap()).unwrap();
    }

    fn sample_history(file: &Path, index: usize, name: &str, url: &str) -> HistoryEntry {
        HistoryEntry {
            time: "2026-11-19T10:20:30+08:00".to_owned(),
            file: file.to_string_lossy().into_owned(),
            request_index: index,
            name: name.to_owned(),
            method: "GET".to_owned(),
            url: url.to_owned(),
            status: 200,
            duration_ms: 12,
            error: None,
        }
    }

    /// 载入工作区时读取历史，搜索关键字可过滤方法 / URL / 名称
    #[test]
    fn history_loads_and_filters() {
        let (dir, a, _b) = tab_workspace("hist-filter");
        write_history(
            &dir,
            &[
                sample_history(&a, 0, "用户列表", "https://example.com/users"),
                sample_history(&a, 1, "订单", "https://example.com/orders"),
            ],
        );
        let mut app = HttpDeckApp::new();
        app.load_workspace(dir.clone());
        assert_eq!(app.history.len(), 2, "载入工作区应读取历史");

        assert!(history_matches(&app.history[0], ""));
        assert!(history_matches(&app.history[0], "USERS"));
        // 文件路径本身含用户名（如 C:\\Users\\...），这里用 URL 独有的关键字做负向断言
        assert!(!history_matches(&app.history[0], "orders"));
        assert!(history_matches(&app.history[1], "orders"));
        assert!(history_matches(&app.history[0], "200"));

        app.history_search = "订单".to_owned();
        let filtered: Vec<&HistoryEntry> = app
            .history
            .iter()
            .filter(|entry| history_matches(entry, &app.history_search))
            .collect();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "订单");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// client.global 变量应能展示并一键清空，且同步写入磁盘
    #[test]
    fn globals_list_and_clear() {
        let (dir, _a, _b) = tab_workspace("globals");
        let store = dir.join(".http-client").join("globals.json");
        std::fs::create_dir_all(store.parent().unwrap()).unwrap();
        std::fs::write(&store, r#"{"token":"abc"}"#).unwrap();

        let mut app = HttpDeckApp::new();
        app.load_workspace(dir.clone());
        assert_eq!(app.globals.len(), 1, "载入工作区应读取 globals");
        assert_eq!(app.globals[0].name, "token");
        assert_eq!(app.globals[0].value, "abc");

        app.requests_tab = RequestsTab::Globals;
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 820.0),
            )),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            app.draw(ui);
        });
        output.textures_delta.clear();
        let texts: Vec<String> = output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|text| text == "token"), "{texts:?}");
        assert!(texts.iter().any(|text| text == "abc"), "{texts:?}");

        app.clear_globals();
        assert!(app.globals.is_empty());
        let text = std::fs::read_to_string(&store).unwrap();
        assert_eq!(text.trim(), "{}", "清空应写入磁盘");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 历史重发应打开对应文件并选中请求；清空后内存与磁盘均为空
    #[test]
    fn history_rerun_selects_request_and_clear_works() {
        let (dir, a, _b) = tab_workspace("hist-rerun");
        std::fs::write(
            &a,
            "@host = http://127.0.0.1:1\n\n### 一\nGET {{host}}/one\n\n### 二\nGET {{host}}/two\n",
        )
        .unwrap();
        let entry = sample_history(&a, 1, "二", "http://127.0.0.1:1/two");
        let mut app = HttpDeckApp::new();
        app.load_workspace(dir.clone());

        assert!(app.open_history_entry(&entry), "历史应能定位到请求");
        assert_eq!(app.open_file.as_deref(), Some(a.as_path()));
        assert_eq!(app.selected_request, Some(1));

        let mut missing = entry.clone();
        missing.request_index = 9;
        assert!(!app.open_history_entry(&missing), "请求不存在时应失败");
        assert!(app.error.is_some());

        write_history(&dir, &[entry]);
        app.refresh_history();
        assert_eq!(app.history.len(), 1);
        app.clear_history();
        assert!(app.history.is_empty());
        assert!(history::list(&dir).is_empty(), "清空应写入磁盘");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 切换到历史标签后，右侧面板不应被长文本撑宽而覆盖编辑区
    #[test]
    fn history_tab_keeps_panel_width() {
        let (dir, a, _b) = tab_workspace("hist-width");
        let long_name = "超长请求名称".repeat(20);
        let long_url = format!("https://example.com/{}", "very-long-path/".repeat(20));
        write_history(&dir, &[sample_history(&a, 0, &long_name, &long_url)]);
        let mut app = HttpDeckApp::new();
        app.load_workspace(dir.clone());
        app.load_file(a.clone());
        app.requests_tab = RequestsTab::History;

        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 820.0),
            )),
            ..Default::default()
        };
        for frame in 0..5 {
            let mut rects = None;
            let mut output = ctx.run_ui(input.clone(), |ui| {
                rects = Some(app.draw(ui));
            });
            output.textures_delta.clear();
            let rects = rects.expect("draw 未执行");
            println!("history frame {frame}: {rects:?}");
            assert!(
                rects.requests.width() < 400.0,
                "历史面板不应被长文本撑宽: {:?}",
                rects.requests
            );
            assert!(
                rects.central.right() <= rects.requests.left() + 1.0,
                "历史面板不应覆盖编辑区: {:?} vs {:?}",
                rects.central,
                rects.requests
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 请求列表项右键应弹出包含「运行」的菜单
    #[test]
    fn requests_list_has_run_context_menu() {
        let (dir, a, _b) = tab_workspace("req-menu");
        let mut app = HttpDeckApp::new();
        app.load_workspace(dir.clone());
        app.load_file(a.clone());

        let ctx = egui::Context::default();
        let mut draw = |events: Vec<egui::Event>| -> (Vec<String>, Option<egui::Pos2>) {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 820.0),
                )),
                events,
                ..Default::default()
            };
            let mut rects = None;
            let mut output = ctx.run_ui(input, |ui| {
                rects = Some(app.draw(ui));
            });
            output.textures_delta.clear();
            let requests_left = rects.map_or(0.0, |rects| rects.requests.left());
            // 只在右侧请求面板范围内找列表项文本，排除编辑区里的同一行
            let item_pos = output.shapes.iter().find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text)
                    if text.galley.text().starts_with("GET") && text.pos.x >= requests_left =>
                {
                    Some(text.pos + egui::vec2(4.0, 4.0))
                }
                _ => None,
            });
            let texts = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
            (texts, item_pos)
        };
        let (_, item_pos) = draw(Vec::new());
        let pos = item_pos.expect("未找到请求列表项");
        draw(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Secondary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        draw(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);
        let (texts, _) = draw(Vec::new());
        assert!(ctx.any_popup_open(), "请求右键应弹出菜单");
        assert!(
            texts.iter().any(|text| text == "运行"),
            "请求菜单应包含运行: {texts:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
