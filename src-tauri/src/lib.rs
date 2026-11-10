mod cookies;
mod envfile;
mod executor;
mod history;
mod models;
mod parser;
mod recent;
mod script;
mod variables;
mod workspace;

use cookies::SharedCookieStore;
use models::{
    CookieInfo, FileRunResult, HistoryEntry, KeyValue, ParsedFileOutput, RequestResult,
    WorkspaceInfo,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tauri::State;

pub struct WorkspaceRuntime {
    cookies: Arc<SharedCookieStore>,
    globals: HashMap<String, String>,
}

#[derive(Default)]
pub struct AppState {
    workspaces: Mutex<HashMap<String, WorkspaceRuntime>>,
    run_lock: tokio::sync::Mutex<()>,
    cancel_token: Mutex<Option<Arc<executor::CancelToken>>>,
}

fn install_cancel_token(state: &AppState) -> Arc<executor::CancelToken> {
    let token = executor::CancelToken::new();
    if let Ok(mut guard) = state.cancel_token.lock() {
        *guard = Some(token.clone());
    }
    token
}

fn clear_cancel_token(state: &AppState) {
    if let Ok(mut guard) = state.cancel_token.lock() {
        *guard = None;
    }
}

#[tauri::command]
fn cancel_run(state: State<'_, AppState>) {
    if let Ok(guard) = state.cancel_token.lock() {
        if let Some(token) = guard.as_ref() {
            token.cancel();
        }
    }
}

#[tauri::command]
fn reveal_in_explorer(path: String) -> Result<(), String> {
    let target = PathBuf::from(&path);
    if !target.exists() {
        return Err(format!("路径不存在: {path}"));
    }
    // Windows: /select 定位文件，目录直接打开
    let status = if target.is_dir() {
        std::process::Command::new("explorer")
            .arg(&path)
            .status()
    } else {
        std::process::Command::new("explorer")
            .arg(format!("/select,{path}"))
            .status()
    };
    status.map_err(|e| format!("打开资源管理器失败: {e}"))?;
    Ok(())
}

#[tauri::command]
fn list_history(workspace: String) -> Result<Vec<HistoryEntry>, String> {
    let root = canonical_root(&workspace)?;
    Ok(history::list(&root))
}

#[tauri::command]
fn clear_history(workspace: String) -> Result<(), String> {
    let root = canonical_root(&workspace)?;
    history::clear(&root);
    Ok(())
}

fn canonical_root(path: &str) -> Result<PathBuf, String> {
    let root = std::fs::canonicalize(path).map_err(|e| format!("无法打开目录 {path}: {e}"))?;
    if !root.is_dir() {
        return Err(format!("不是目录: {path}"));
    }
    Ok(root)
}

fn runtime_key(root: &Path) -> String {
    root.to_string_lossy().to_string()
}

fn snapshot_runtime(
    state: &AppState,
    root: &Path,
) -> Result<(Arc<SharedCookieStore>, HashMap<String, String>), String> {
    let key = runtime_key(root);
    let guard = state
        .workspaces
        .lock()
        .map_err(|_| "工作区状态锁失败".to_string())?;
    let runtime = guard
        .get(&key)
        .ok_or_else(|| "工作区尚未打开，请重新打开目录".to_string())?;
    Ok((runtime.cookies.clone(), runtime.globals.clone()))
}

fn store_globals(state: &AppState, root: &Path, globals: HashMap<String, String>) {
    let key = runtime_key(root);
    if let Ok(mut guard) = state.workspaces.lock() {
        if let Some(runtime) = guard.get_mut(&key) {
            runtime.globals = globals;
        }
    }
}

fn build_workspace_info(state: &AppState, root: &Path) -> Result<WorkspaceInfo, String> {
    let config = workspace::load_config(root);
    let envs = envfile::load_envs(root);
    let selected_env = config
        .selected_env
        .clone()
        .filter(|env| envs.names.iter().any(|name| name == env))
        .or_else(|| envs.names.first().cloned());
    let pinned = workspace::valid_pinned(root, &config.pinned);
    let (_, globals) = snapshot_runtime(state, root)?;
    let mut global_list: Vec<KeyValue> = globals
        .into_iter()
        .map(|(name, value)| KeyValue { name, value })
        .collect();
    global_list.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(WorkspaceInfo {
        root: root.to_string_lossy().to_string(),
        name: root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| root.to_string_lossy().to_string()),
        tree: workspace::scan_tree(root),
        pinned,
        envs: envs.names,
        selected_env,
        globals: global_list,
        proxy: config.proxy.clone(),
    })
}

#[tauri::command]
fn initial_workspace() -> Option<String> {
    std::env::args()
        .skip(1)
        .find(|arg| Path::new(arg).is_dir())
}

#[tauri::command]
fn recent_workspaces() -> Vec<String> {
    recent::list()
}

#[tauri::command]
fn open_workspace(state: State<'_, AppState>, path: String) -> Result<WorkspaceInfo, String> {
    let root = canonical_root(&path)?;
    let key = runtime_key(&root);
    {
        let mut guard = state
            .workspaces
            .lock()
            .map_err(|_| "工作区状态锁失败".to_string())?;
        guard.entry(key).or_insert_with(|| WorkspaceRuntime {
            cookies: cookies::load(&root),
            globals: HashMap::new(),
        });
    }
    let info = build_workspace_info(&state, &root)?;
    recent::record(&root);
    Ok(info)
}

#[tauri::command]
fn close_workspace(state: State<'_, AppState>, workspace: String) -> Result<(), String> {
    let root = std::fs::canonicalize(&workspace).unwrap_or_else(|_| PathBuf::from(&workspace));
    if let Ok(mut guard) = state.workspaces.lock() {
        guard.remove(&runtime_key(&root));
    }
    recent::forget(&root);
    Ok(())
}

#[tauri::command]
fn refresh_workspace(state: State<'_, AppState>, workspace: String) -> Result<WorkspaceInfo, String> {
    let root = canonical_root(&workspace)?;
    build_workspace_info(&state, &root)
}

#[tauri::command]
fn read_http_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| format!("读取文件失败 {path}: {e}"))
}

#[tauri::command]
fn save_http_file(path: String, content: String) -> Result<(), String> {
    std::fs::write(&path, content).map_err(|e| format!("保存文件失败 {path}: {e}"))
}

#[tauri::command]
fn parse_http_file(path: String) -> Result<ParsedFileOutput, String> {
    let content = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取文件失败 {path}: {e}"))?;
    Ok(parser::parse(&content).to_output())
}

#[tauri::command]
fn parse_http_text(content: String) -> ParsedFileOutput {
    parser::parse(&content).to_output()
}

#[tauri::command]
fn set_pinned(
    state: State<'_, AppState>,
    workspace: String,
    file: String,
    pinned: bool,
) -> Result<WorkspaceInfo, String> {
    let root = canonical_root(&workspace)?;
    let rel = if Path::new(&file).is_absolute() {
        workspace::rel_path(&root, Path::new(&file))
    } else {
        workspace::normalize_rel(&file)
    };
    let mut config = workspace::load_config(&root);
    if pinned {
        if !config.pinned.contains(&rel) {
            config.pinned.push(rel);
        }
    } else {
        config.pinned.retain(|item| item != &rel);
    }
    workspace::save_config(&root, &config)?;
    build_workspace_info(&state, &root)
}

#[tauri::command]
fn set_env(
    state: State<'_, AppState>,
    workspace: String,
    env: Option<String>,
) -> Result<WorkspaceInfo, String> {
    let root = canonical_root(&workspace)?;
    let mut config = workspace::load_config(&root);
    config.selected_env = env.filter(|value| !value.trim().is_empty());
    workspace::save_config(&root, &config)?;
    build_workspace_info(&state, &root)
}

#[tauri::command]
fn set_proxy(
    state: State<'_, AppState>,
    workspace: String,
    proxy: Option<String>,
) -> Result<WorkspaceInfo, String> {
    let root = canonical_root(&workspace)?;
    let mut config = workspace::load_config(&root);
    config.proxy = proxy.filter(|value| !value.trim().is_empty());
    workspace::save_config(&root, &config)?;
    build_workspace_info(&state, &root)
}

#[tauri::command]
fn get_globals(state: State<'_, AppState>, workspace: String) -> Result<Vec<KeyValue>, String> {
    let root = canonical_root(&workspace)?;
    let (_, globals) = snapshot_runtime(&state, &root)?;
    let mut list: Vec<KeyValue> = globals
        .into_iter()
        .map(|(name, value)| KeyValue { name, value })
        .collect();
    list.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(list)
}

#[tauri::command]
fn clear_globals(state: State<'_, AppState>, workspace: String) -> Result<(), String> {
    let root = canonical_root(&workspace)?;
    store_globals(&state, &root, HashMap::new());
    Ok(())
}

fn env_vars_for(root: &Path, dir: &Path, env: Option<&str>) -> HashMap<String, String> {
    // .env 作为基础层，具名环境变量覆盖其上
    let mut vars = envfile::load_dotenv_chain(root, dir);
    if let Some(env) = env.filter(|value| !value.trim().is_empty()) {
        for (key, value) in envfile::load_envs_chain(root, dir).vars_for(env) {
            vars.insert(key, value);
        }
    }
    vars
}

fn resolve_env(root: &Path, requested: Option<String>) -> Option<String> {
    if let Some(env) = requested.filter(|value| !value.trim().is_empty()) {
        return Some(env);
    }
    workspace::load_config(root).selected_env
}

fn read_parsed(path: &str) -> Result<parser::ParsedHttpFile, String> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("读取文件失败 {path}: {e}"))?;
    Ok(parser::parse(&content))
}

fn file_dir(path: &str) -> PathBuf {
    Path::new(path)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

#[tauri::command]
async fn run_request(
    state: State<'_, AppState>,
    workspace: String,
    file: String,
    index: usize,
    env: Option<String>,
) -> Result<RequestResult, String> {
    let _guard = state.run_lock.lock().await;
    let root = canonical_root(&workspace)?;
    let (cookies, mut globals) = snapshot_runtime(&state, &root)?;
    let parsed = read_parsed(&file)?;
    let def = parsed
        .requests
        .get(index)
        .ok_or_else(|| format!("请求 #{index} 不存在"))?;
    let env_name = resolve_env(&root, env);
    let env_vars = env_vars_for(&root, &file_dir(&file), env_name.as_deref());
    let file_vars: HashMap<String, String> = parsed.variables.into_iter().collect();
    let token = install_cancel_token(&state);
    let proxy = workspace::load_config(&root).proxy;
    let result = executor::execute_request(
        def,
        &file_vars,
        &env_vars,
        &mut globals,
        cookies.clone(),
        &file_dir(&file),
        Some(token),
        proxy.as_deref(),
    )
    .await;
    clear_cancel_token(&state);
    history::record(&root, &file, &result);
    cookies::save(&root, &cookies);
    store_globals(&state, &root, globals);
    Ok(result)
}

#[tauri::command]
async fn run_file(
    state: State<'_, AppState>,
    workspace: String,
    file: String,
    env: Option<String>,
) -> Result<Vec<RequestResult>, String> {
    let _guard = state.run_lock.lock().await;
    let root = canonical_root(&workspace)?;
    let (cookies, mut globals) = snapshot_runtime(&state, &root)?;
    let parsed = read_parsed(&file)?;
    let env_name = resolve_env(&root, env);
    let dir = file_dir(&file);
    let env_vars = env_vars_for(&root, &dir, env_name.as_deref());
    let file_vars: HashMap<String, String> = parsed.variables.into_iter().collect();
    let token = install_cancel_token(&state);
    let proxy = workspace::load_config(&root).proxy;
    let mut results = Vec::new();
    for def in &parsed.requests {
        if token.is_cancelled() {
            break;
        }
        let result = executor::execute_request(
            def,
            &file_vars,
            &env_vars,
            &mut globals,
            cookies.clone(),
            &dir,
            Some(token.clone()),
            proxy.as_deref(),
        )
        .await;
        history::record(&root, &file, &result);
        results.push(result);
    }
    clear_cancel_token(&state);
    cookies::save(&root, &cookies);
    store_globals(&state, &root, globals);
    Ok(results)
}

#[tauri::command]
async fn run_folder(
    state: State<'_, AppState>,
    workspace: String,
    folder: String,
    env: Option<String>,
) -> Result<Vec<FileRunResult>, String> {
    let _guard = state.run_lock.lock().await;
    let root = canonical_root(&workspace)?;
    let (cookies, mut globals) = snapshot_runtime(&state, &root)?;
    let env_name = resolve_env(&root, env);
    let files = workspace::collect_http_files(Path::new(&folder));
    let token = install_cancel_token(&state);
    let proxy = workspace::load_config(&root).proxy;
    let mut output = Vec::new();
    for path in files {
        if token.is_cancelled() {
            break;
        }
        let file_string = path.to_string_lossy().to_string();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| file_string.clone());
        match read_parsed(&file_string) {
            Ok(parsed) => {
                let dir = file_dir(&file_string);
                // 目录级环境：每个文件按自己的目录链解析
                let env_vars = env_vars_for(&root, &dir, env_name.as_deref());
                let file_vars: HashMap<String, String> = parsed.variables.into_iter().collect();
                let mut results = Vec::new();
                for def in &parsed.requests {
                    if token.is_cancelled() {
                        break;
                    }
                    let result = executor::execute_request(
                        def,
                        &file_vars,
                        &env_vars,
                        &mut globals,
                        cookies.clone(),
                        &dir,
                        Some(token.clone()),
                        proxy.as_deref(),
                    )
                    .await;
                    history::record(&root, &file_string, &result);
                    results.push(result);
                }
                output.push(FileRunResult {
                    file: file_string,
                    name,
                    results,
                    error: None,
                });
            }
            Err(err) => output.push(FileRunResult {
                file: file_string,
                name,
                results: Vec::new(),
                error: Some(err),
            }),
        }
    }
    clear_cancel_token(&state);
    cookies::save(&root, &cookies);
    store_globals(&state, &root, globals);
    Ok(output)
}

#[tauri::command]
fn list_cookies(state: State<'_, AppState>, workspace: String) -> Result<Vec<CookieInfo>, String> {
    let root = canonical_root(&workspace)?;
    let (cookies, _) = snapshot_runtime(&state, &root)?;
    Ok(cookies::list(&cookies))
}

#[tauri::command]
fn clear_cookies(state: State<'_, AppState>, workspace: String) -> Result<(), String> {
    let root = canonical_root(&workspace)?;
    let (cookies, _) = snapshot_runtime(&state, &root)?;
    cookies::clear(&root, &cookies);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            initial_workspace,
            recent_workspaces,
            open_workspace,
            close_workspace,
            refresh_workspace,
            read_http_file,
            save_http_file,
            parse_http_file,
            parse_http_text,
            set_pinned,
            set_env,
            set_proxy,
            get_globals,
            clear_globals,
            run_request,
            run_file,
            run_folder,
            cancel_run,
            list_history,
            clear_history,
            list_cookies,
            clear_cookies,
            reveal_in_explorer
        ])
        .run(tauri::generate_context!())
        .expect("error while running HttpDeck");
}
