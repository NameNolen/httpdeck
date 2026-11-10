use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RequestOptions {
    pub no_redirect: bool,
    pub no_cookie_jar: bool,
    pub no_log: bool,
    pub timeout_ms: Option<u64>,
    pub accept_invalid_certs: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyValue {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestSummary {
    pub index: usize,
    pub name: String,
    pub method: String,
    pub url: String,
    pub line: usize,
    pub has_pre_script: bool,
    pub has_post_script: bool,
    pub body_kind: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedFileOutput {
    pub requests: Vec<RequestSummary>,
    pub variables: Vec<KeyValue>,
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestResult {
    pub request_index: usize,
    pub name: String,
    pub method: String,
    pub url: String,
    pub request_headers: Vec<KeyValue>,
    pub request_body: Option<String>,
    pub status: u16,
    pub status_text: String,
    pub response_headers: Vec<KeyValue>,
    pub body: String,
    pub body_is_binary: bool,
    pub mime_type: Option<String>,
    pub size: usize,
    pub duration_ms: u128,
    pub tests: Vec<TestResult>,
    pub logs: Vec<String>,
    pub script_error: Option<String>,
    pub saved_to: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRunResult {
    pub file: String,
    pub name: String,
    pub results: Vec<RequestResult>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileNode {
    pub name: String,
    pub path: String,
    pub rel_path: String,
    pub is_dir: bool,
    pub children: Vec<FileNode>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInfo {
    pub root: String,
    pub name: String,
    pub tree: FileNode,
    pub pinned: Vec<String>,
    pub envs: Vec<String>,
    pub selected_env: Option<String>,
    pub globals: Vec<KeyValue>,
    pub proxy: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CookieInfo {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub expires: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct WorkspaceConfig {
    pub pinned: Vec<String>,
    pub selected_env: Option<String>,
    /// 工作区级代理，例如 http://127.0.0.1:7890 或 socks5://127.0.0.1:1080
    pub proxy: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub time: String,
    pub file: String,
    pub request_index: usize,
    pub name: String,
    pub method: String,
    pub url: String,
    pub status: u16,
    pub duration_ms: u128,
    pub error: Option<String>,
}
