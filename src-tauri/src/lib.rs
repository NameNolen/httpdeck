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

fn recent_files_path(root: &Path) -> PathBuf {
    root.join(".http-client").join("recent-files.json")
}

fn load_recent_files(root: &Path) -> Vec<String> {
    std::fs::read_to_string(recent_files_path(root))
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<String>>(&text).ok())
        .unwrap_or_default()
}

fn save_recent_files(root: &Path, files: &[String]) -> Result<(), String> {
    let path = recent_files_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建目录失败 {}: {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(files)
        .map_err(|e| format!("序列化最近文件失败: {e}"))?;
    std::fs::write(&path, text)
        .map_err(|e| format!("保存最近文件失败 {}: {e}", path.display()))
}

fn absolute_recent_files(root: &Path, files: &[String]) -> Vec<String> {
    files
        .iter()
        .map(|rel| root.join(rel).to_string_lossy().to_string())
        .filter(|path| Path::new(path).is_file())
        .collect()
}

#[tauri::command]
fn list_recent_files(workspace: String) -> Result<Vec<String>, String> {
    let root = canonical_root(&workspace)?;
    Ok(absolute_recent_files(&root, &load_recent_files(&root)))
}

#[tauri::command]
fn record_recent_file(workspace: String, file: String) -> Result<Vec<String>, String> {
    let root = canonical_root(&workspace)?;
    let file = std::fs::canonicalize(&file).unwrap_or_else(|_| PathBuf::from(&file));
    let rel = workspace::rel_path(&root, &file);
    let mut files = load_recent_files(&root);
    files.retain(|item| item != &rel);
    files.insert(0, rel);
    files.truncate(10);
    save_recent_files(&root, &files)?;
    Ok(absolute_recent_files(&root, &files))
}

#[tauri::command]
fn clear_recent_files(workspace: String) -> Result<(), String> {
    let root = canonical_root(&workspace)?;
    save_recent_files(&root, &[])
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

/// 校验用户输入的文件名，不允许路径分隔符与上级目录引用
fn validate_file_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("文件名不能为空".to_string());
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err("文件名不能包含路径分隔符或 ..".to_string());
    }
    Ok(name.to_string())
}

/// 新建文件时未写扩展名则补 `.http`
fn with_http_extension(name: &str) -> String {
    if name.contains('.') {
        name.to_string()
    } else {
        format!("{name}.http")
    }
}

#[tauri::command]
fn create_http_file(
    state: State<'_, AppState>,
    workspace: String,
    parent: String,
    name: String,
) -> Result<(WorkspaceInfo, String), String> {
    let root = canonical_root(&workspace)?;
    let name = with_http_extension(&validate_file_name(&name)?);
    let dir = if parent.trim().is_empty() {
        root.clone()
    } else {
        PathBuf::from(&parent)
    };
    if !dir.is_dir() {
        return Err(format!("目录不存在: {}", dir.display()));
    }
    let target = dir.join(&name);
    if target.exists() {
        return Err(format!("文件已存在: {name}"));
    }
    std::fs::write(&target, "### 新建请求\nGET https://example.com\n\n")
        .map_err(|e| format!("创建文件失败 {}: {e}", target.display()))?;
    let info = build_workspace_info(&state, &root)?;
    Ok((info, target.to_string_lossy().to_string()))
}

#[tauri::command]
fn rename_http_file(
    state: State<'_, AppState>,
    workspace: String,
    file: String,
    name: String,
) -> Result<(WorkspaceInfo, String), String> {
    let root = canonical_root(&workspace)?;
    let source = PathBuf::from(&file);
    if !source.is_file() {
        return Err(format!("文件不存在: {file}"));
    }
    let mut new_name = validate_file_name(&name)?;
    if !new_name.contains('.') {
        if let Some(ext) = source.extension() {
            new_name.push('.');
            new_name.push_str(&ext.to_string_lossy());
        }
    }
    let dir = source
        .parent()
        .ok_or_else(|| "无法确定文件所在目录".to_string())?;
    let target = dir.join(&new_name);
    if target != source && target.exists() {
        return Err(format!("目标文件已存在: {new_name}"));
    }
    std::fs::rename(&source, &target).map_err(|e| format!("重命名失败: {e}"))?;

    // 同步置顶列表中的相对路径
    let old_rel = workspace::rel_path(&root, &source);
    let new_rel = workspace::rel_path(&root, &target);
    let mut config = workspace::load_config(&root);
    if let Some(slot) = config.pinned.iter_mut().find(|item| **item == old_rel) {
        *slot = new_rel.clone();
        workspace::save_config(&root, &config)?;
    }
    // 同步最近打开列表
    let mut recent = load_recent_files(&root);
    if let Some(slot) = recent.iter_mut().find(|item| **item == old_rel) {
        *slot = new_rel;
        save_recent_files(&root, &recent)?;
    }
    let info = build_workspace_info(&state, &root)?;
    Ok((info, target.to_string_lossy().to_string()))
}

#[tauri::command]
fn delete_http_file(
    state: State<'_, AppState>,
    workspace: String,
    file: String,
) -> Result<WorkspaceInfo, String> {
    let root = canonical_root(&workspace)?;
    let target = PathBuf::from(&file);
    if !target.is_file() {
        return Err(format!("文件不存在: {file}"));
    }
    std::fs::remove_file(&target).map_err(|e| format!("删除文件失败: {e}"))?;
    let rel = workspace::rel_path(&root, &target);
    let mut config = workspace::load_config(&root);
    let before = config.pinned.len();
    config.pinned.retain(|item| item != &rel);
    if config.pinned.len() != before {
        workspace::save_config(&root, &config)?;
    }
    // 从最近打开列表移除
    let mut recent = load_recent_files(&root);
    let before_recent = recent.len();
    recent.retain(|item| item != &rel);
    if recent.len() != before_recent {
        save_recent_files(&root, &recent)?;
    }
    build_workspace_info(&state, &root)
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

/// 列出指定文件目录层级下当前环境生效的变量，用于编辑器补全。
#[tauri::command]
fn list_variables(
    workspace: String,
    file: String,
    env: Option<String>,
) -> Result<Vec<KeyValue>, String> {
    let root = canonical_root(&workspace)?;
    let env_name = resolve_env(&root, env);
    let mut list: Vec<KeyValue> = env_vars_for(&root, &file_dir(&file), env_name.as_deref())
        .into_iter()
        .map(|(name, value)| KeyValue { name, value })
        .collect();
    list.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(list)
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
            create_http_file,
            rename_http_file,
            delete_http_file,
            parse_http_file,
            parse_http_text,
            set_pinned,
            set_env,
            set_proxy,
            get_globals,
            clear_globals,
            list_variables,
            run_request,
            run_file,
            run_folder,
            cancel_run,
            list_history,
            clear_history,
            list_recent_files,
            record_recent_file,
            clear_recent_files,
            list_cookies,
            clear_cookies,
            reveal_in_explorer
        ])
        .run(tauri::generate_context!())
        .expect("error while running HttpDeck");
}

#[cfg(test)]
mod tests {
    use super::{validate_file_name, with_http_extension};
    use std::path::Path;

    #[test]
    fn validates_and_completes_file_names() {
        assert_eq!(validate_file_name("  demo  ").unwrap(), "demo");
        assert!(validate_file_name("   ").is_err());
        assert!(validate_file_name("../demo").is_err());
        assert!(validate_file_name("dir/demo.http").is_err());
        assert_eq!(with_http_extension("demo"), "demo.http");
        assert_eq!(with_http_extension("demo.rest"), "demo.rest");
    }

    #[test]
    fn records_recent_files_uniquely() {
        let dir = std::env::temp_dir().join(format!(
            "httpdeck-recent-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.http");
        std::fs::write(&file, "### a").unwrap();
        let root = std::fs::canonicalize(&dir).unwrap();
        let workspace = root.to_string_lossy().to_string();
        let file_path = std::fs::canonicalize(&file)
            .unwrap()
            .to_string_lossy()
            .to_string();

        let first = super::record_recent_file(workspace.clone(), file_path.clone()).unwrap();
        assert_eq!(first.len(), 1);
        let second = super::record_recent_file(workspace, file_path).unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(Path::new(&second[0]), root.join("a.http"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
