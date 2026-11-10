use crate::models::{HistoryEntry, RequestResult};
use std::path::{Path, PathBuf};

const MAX_HISTORY: usize = 100;

fn store_path(root: &Path) -> PathBuf {
    root.join(".http-client").join("history.json")
}

fn read_all(root: &Path) -> Vec<HistoryEntry> {
    let Ok(text) = std::fs::read_to_string(store_path(root)) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn write_all(root: &Path, items: &[HistoryEntry]) {
    let path = store_path(root);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(items) {
        let _ = std::fs::write(path, text);
    }
}

/// 列出工作区请求历史，新的在前。
pub fn list(root: &Path) -> Vec<HistoryEntry> {
    read_all(root)
}

/// 记录一次请求执行结果；取消的请求不记入历史。
pub fn record(root: &Path, file: &str, result: &RequestResult) {
    if result.error.as_deref() == Some("请求已取消") {
        return;
    }
    let mut items = read_all(root);
    items.insert(
        0,
        HistoryEntry {
            time: chrono::Local::now().to_rfc3339(),
            file: file.to_string(),
            request_index: result.request_index,
            name: result.name.clone(),
            method: result.method.clone(),
            url: result.url.clone(),
            status: result.status,
            duration_ms: result.duration_ms,
            error: result.error.clone(),
        },
    );
    items.truncate(MAX_HISTORY);
    write_all(root, &items);
}

pub fn clear(root: &Path) {
    write_all(root, &[]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_result() -> RequestResult {
        RequestResult {
            request_index: 2,
            name: "查询用户".to_string(),
            method: "GET".to_string(),
            url: "http://localhost/users".to_string(),
            request_headers: Vec::new(),
            request_body: None,
            status: 200,
            status_text: "OK".to_string(),
            response_headers: Vec::new(),
            body: String::new(),
            body_is_binary: false,
            mime_type: None,
            size: 0,
            duration_ms: 12,
            tests: Vec::new(),
            logs: Vec::new(),
            script_error: None,
            saved_to: None,
            error: None,
        }
    }

    #[test]
    fn records_and_truncates_history() {
        let base =
            std::env::temp_dir().join(format!("httpdeck-history-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();

        assert!(list(&base).is_empty());
        record(&base, "a.http", &sample_result());
        record(&base, "b.http", &sample_result());
        let items = list(&base);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].file, "b.http");
        assert_eq!(items[0].request_index, 2);
        assert_eq!(items[0].status, 200);

        // 取消的请求不记录
        let mut cancelled = sample_result();
        cancelled.error = Some("请求已取消".to_string());
        record(&base, "c.http", &cancelled);
        assert_eq!(list(&base).len(), 2);

        clear(&base);
        assert!(list(&base).is_empty());

        std::fs::remove_dir_all(&base).ok();
    }
}
