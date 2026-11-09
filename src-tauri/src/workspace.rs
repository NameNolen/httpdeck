use crate::models::{FileNode, WorkspaceConfig};
use std::path::{Path, PathBuf};

const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "build",
    ".git",
    ".idea",
    ".vscode",
    ".http-client",
    ".screenshots",
];

pub fn config_path(root: &Path) -> PathBuf {
    root.join(".http-client").join("workspace.json")
}

pub fn load_config(root: &Path) -> WorkspaceConfig {
    let path = config_path(root);
    let Ok(text) = std::fs::read_to_string(path) else {
        return WorkspaceConfig::default();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

pub fn save_config(root: &Path, config: &WorkspaceConfig) -> Result<(), String> {
    let path = config_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建配置目录失败: {e}"))?;
    }
    let text = serde_json::to_string_pretty(config).map_err(|e| format!("序列化配置失败: {e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("写入配置失败: {e}"))
}

pub fn is_http_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("http") || e.eq_ignore_ascii_case("rest"))
        .unwrap_or(false)
}

pub fn normalize_rel(path: &str) -> String {
    path.replace('\\', "/")
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string()
}

pub fn rel_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|p| normalize_rel(&p.to_string_lossy()))
        .unwrap_or_else(|_| normalize_rel(&path.to_string_lossy()))
}

pub fn scan_tree(root: &Path) -> FileNode {
    scan_dir(root, root, 0).unwrap_or_else(|| FileNode {
        name: root
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| root.to_string_lossy().to_string()),
        path: root.to_string_lossy().to_string(),
        rel_path: String::new(),
        is_dir: true,
        children: Vec::new(),
    })
}

fn scan_dir(dir: &Path, root: &Path, depth: usize) -> Option<FileNode> {
    if depth > 16 {
        return None;
    }
    let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name().to_string_lossy().to_lowercase());

    let mut children = Vec::new();
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.to_lowercase().as_str()) {
            continue;
        }
        let file_type = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        let path = entry.path();
        if file_type.is_dir() {
            if let Some(node) = scan_dir(&path, root, depth + 1) {
                children.push(node);
            }
        } else if file_type.is_file() && is_http_file(&path) {
            children.push(FileNode {
                name,
                path: path.to_string_lossy().to_string(),
                rel_path: rel_path(root, &path),
                is_dir: false,
                children: Vec::new(),
            });
        }
    }

    children.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    if depth > 0 && children.is_empty() {
        return None;
    }

    Some(FileNode {
        name: dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| dir.to_string_lossy().to_string()),
        path: dir.to_string_lossy().to_string(),
        rel_path: rel_path(root, dir),
        is_dir: true,
        children,
    })
}

pub fn collect_http_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_recursive(dir, &mut files, 0);
    files.sort_by_key(|p| p.to_string_lossy().to_lowercase());
    files
}

fn collect_recursive(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 16 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.to_lowercase().as_str()) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if file_type.is_dir() {
            collect_recursive(&path, out, depth + 1);
        } else if file_type.is_file() && is_http_file(&path) {
            out.push(path);
        }
    }
}

pub fn valid_pinned(root: &Path, pinned: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    for item in pinned {
        let rel = normalize_rel(item);
        if rel.is_empty() {
            continue;
        }
        let candidate = root.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        if candidate.is_file() && !result.contains(&rel) {
            result.push(rel);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_relative_paths() {
        assert_eq!(normalize_rel(".\\api\\users.http"), "api/users.http");
        assert_eq!(normalize_rel("./api/users.http"), "api/users.http");
    }
}
