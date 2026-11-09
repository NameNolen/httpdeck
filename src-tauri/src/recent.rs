use std::path::{Path, PathBuf};

const MAX_RECENT: usize = 8;

fn store_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from))
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"))
        })
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("HttpDeck").join("recent.json")
}

fn read_all() -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(store_path()) else {
        return Vec::new();
    };
    serde_json::from_str::<Vec<String>>(&text).unwrap_or_default()
}

fn write_all(items: &[String]) {
    let path = store_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(items) {
        let _ = std::fs::write(path, text);
    }
}

/// 读取历史目录，只保留仍然存在的目录并去重。
pub fn list() -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    for path in read_all() {
        if !Path::new(&path).is_dir() || items.contains(&path) {
            continue;
        }
        items.push(path);
    }
    items
}

pub fn record(root: &Path) {
    let key = root.to_string_lossy().to_string();
    let mut items = list();
    items.retain(|item| item != &key);
    items.insert(0, key);
    items.truncate(MAX_RECENT);
    write_all(&items);
}

pub fn forget(root: &Path) {
    let key = root.to_string_lossy().to_string();
    let mut items = list();
    items.retain(|item| item != &key);
    write_all(&items);
}
