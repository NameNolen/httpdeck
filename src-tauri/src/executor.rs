use crate::cookies::SharedCookieStore;
use crate::models::{KeyValue, RequestResult};
use crate::parser::{BodySpec, RequestDef, SaveSpec, ScriptSpec};
use crate::script::{self, ScriptRequest, ScriptResponse};
use crate::variables::{self, VariableContext};
use base64::Engine;
use regex::Regex;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Method, Url};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// 一次运行的取消令牌：取消时会唤醒正在等待的 HTTP 请求。
pub struct CancelToken {
    cancelled: AtomicBool,
    notify: tokio::sync::Notify,
}

impl CancelToken {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            cancelled: AtomicBool::new(false),
            notify: tokio::sync::Notify::new(),
        })
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub async fn notified(&self) {
        self.notify.notified().await;
    }
}

pub async fn execute_request(
    def: &RequestDef,
    file_vars: &HashMap<String, String>,
    env_vars: &HashMap<String, String>,
    globals: &mut HashMap<String, String>,
    cookies: Arc<SharedCookieStore>,
    base_dir: &Path,
    cancel: Option<Arc<CancelToken>>,
) -> RequestResult {
    if cancel.as_ref().map(|t| t.is_cancelled()).unwrap_or(false) {
        return simple_failure(
            def,
            &def.url,
            &[],
            None,
            Vec::new(),
            Vec::new(),
            None,
            "请求已取消".to_string(),
        );
    }
    let empty_vars = HashMap::new();
    let pre_ctx = VariableContext {
        file_vars,
        env_vars,
        globals,
        request_vars: &empty_vars,
    };

    let mut tests = Vec::new();
    let mut logs = Vec::new();
    let mut request_vars: HashMap<String, String> = HashMap::new();
    let mut script_error: Option<String> = None;

    let pre_url = variables::resolve(&def.url, &pre_ctx);
    let pre_headers: Vec<(String, String)> = def
        .headers
        .iter()
        .map(|(n, v)| (n.clone(), variables::resolve(v, &pre_ctx)))
        .collect();
    let pre_body = match &def.body {
        BodySpec::Inline(text) => Some(variables::resolve(text, &pre_ctx)),
        _ => None,
    };

    if !def.pre_scripts.is_empty() {
        match load_scripts(&def.pre_scripts, base_dir) {
            Ok(sources) => {
                let outcome = script::run_scripts(
                    &sources,
                    globals,
                    &request_vars,
                    &ScriptRequest {
                        method: def.method.clone(),
                        url: pre_url.clone(),
                        body: pre_body.clone(),
                    },
                    None,
                );
                *globals = outcome.globals;
                request_vars = outcome.request_vars;
                tests.extend(outcome.tests);
                logs.extend(outcome.logs);
                if let Some(err) = outcome.error {
                    return RequestResult {
                        request_index: def.index,
                        name: def.display_name(),
                        method: def.method.clone(),
                        url: pre_url,
                        request_headers: pre_headers
                            .iter()
                            .map(|(n, v)| KeyValue {
                                name: n.clone(),
                                value: v.clone(),
                            })
                            .collect(),
                        request_body: pre_body,
                        status: 0,
                        status_text: String::new(),
                        response_headers: Vec::new(),
                        body: String::new(),
                        body_is_binary: false,
                        mime_type: None,
                        size: 0,
                        duration_ms: 0,
                        tests,
                        logs,
                        script_error: Some(err),
                        saved_to: None,
                        error: Some("前置脚本执行失败，请求未发送".to_string()),
                    };
                }
            }
            Err(err) => {
                return RequestResult {
                    request_index: def.index,
                    name: def.display_name(),
                    method: def.method.clone(),
                    url: pre_url,
                    request_headers: pre_headers
                        .iter()
                        .map(|(n, v)| KeyValue {
                            name: n.clone(),
                            value: v.clone(),
                        })
                        .collect(),
                    request_body: pre_body,
                    status: 0,
                    status_text: String::new(),
                    response_headers: Vec::new(),
                    body: String::new(),
                    body_is_binary: false,
                    mime_type: None,
                    size: 0,
                    duration_ms: 0,
                    tests,
                    logs,
                    script_error: Some(err.clone()),
                    saved_to: None,
                    error: Some(err),
                };
            }
        }
    }

    let ctx = VariableContext {
        file_vars,
        env_vars,
        globals,
        request_vars: &request_vars,
    };
    let url = variables::resolve(&def.url, &ctx);
    let resolved_headers: Vec<(String, String)> = def
        .headers
        .iter()
        .map(|(n, v)| (n.clone(), variables::resolve(v, &ctx)))
        .collect();
    let body_result = build_body(def, &ctx, base_dir);
    let body_bytes = match body_result {
        Ok(bytes) => bytes,
        Err(err) => {
            return simple_failure(
                def,
                &url,
                &resolved_headers,
                None,
                tests,
                logs,
                script_error,
                err,
            )
        }
    };
    let body_preview = body_bytes.as_ref().map(|bytes| preview_bytes(bytes));

    let method = match Method::from_bytes(def.method.as_bytes()) {
        Ok(method) => method,
        Err(err) => {
            return simple_failure(
                def,
                &url,
                &resolved_headers,
                body_preview,
                tests,
                logs,
                script_error,
                format!("无效的 HTTP 方法: {err}"),
            )
        }
    };
    let parsed_url = match Url::parse(url.trim()) {
        Ok(url) => url,
        Err(err) => {
            return simple_failure(
                def,
                &url,
                &resolved_headers,
                body_preview,
                tests,
                logs,
                script_error,
                format!("URL 无效（需要绝对地址，可使用 {{{{host}}}} 变量）: {err}"),
            )
        }
    };

    let mut header_map = HeaderMap::new();
    let mut has_user_agent = false;
    for (name, value) in &resolved_headers {
        let header_name = match HeaderName::from_bytes(name.as_bytes()) {
            Ok(name) => name,
            Err(err) => {
                return simple_failure(
                    def,
                    &url,
                    &resolved_headers,
                    body_preview,
                    tests,
                    logs,
                    script_error,
                    format!("请求头名称无效 `{name}`: {err}"),
                )
            }
        };
        let header_value = match HeaderValue::from_str(value) {
            Ok(value) => value,
            Err(err) => {
                return simple_failure(
                    def,
                    &url,
                    &resolved_headers,
                    body_preview,
                    tests,
                    logs,
                    script_error,
                    format!("请求头 `{name}` 的值无效: {err}"),
                )
            }
        };
        if header_name == reqwest::header::USER_AGENT {
            has_user_agent = true;
        }
        header_map.append(header_name, header_value);
    }

    let timeout = Duration::from_millis(def.options.timeout_ms.unwrap_or(60_000));
    let redirect = if def.options.no_redirect {
        reqwest::redirect::Policy::none()
    } else {
        reqwest::redirect::Policy::limited(10)
    };
    let mut client_builder = Client::builder()
        .timeout(timeout)
        .redirect(redirect)
        .danger_accept_invalid_certs(def.options.accept_invalid_certs);
    if !def.options.no_cookie_jar {
        client_builder = client_builder.cookie_provider(cookies);
    }
    if !has_user_agent {
        client_builder = client_builder.user_agent("HttpDeck/0.1.0");
    }
    let client = match client_builder.build() {
        Ok(client) => client,
        Err(err) => {
            return simple_failure(
                def,
                &url,
                &resolved_headers,
                body_preview,
                tests,
                logs,
                script_error,
                format!("HTTP 客户端创建失败: {err}"),
            )
        }
    };

    let mut request = client.request(method.clone(), parsed_url).headers(header_map);
    if let Some(bytes) = body_bytes.clone() {
        request = request.body(bytes);
    }

    let started = Instant::now();
    let send_result = match &cancel {
        Some(token) => {
            tokio::select! {
                _ = token.notified() => {
                    return simple_failure(
                        def,
                        &url,
                        &resolved_headers,
                        body_preview,
                        tests,
                        logs,
                        script_error,
                        "请求已取消".to_string(),
                    );
                }
                result = request.send() => result,
            }
        }
        None => request.send().await,
    };
    let response = match send_result {
        Ok(response) => response,
        Err(err) => {
            return simple_failure(
                def,
                &url,
                &resolved_headers,
                body_preview,
                tests,
                logs,
                script_error,
                format!("请求失败: {err}"),
            )
        }
    };
    let elapsed = started.elapsed().as_millis();
    let status = response.status().as_u16();
    let status_text = response
        .status()
        .canonical_reason()
        .unwrap_or_default()
        .to_string();
    let response_headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.to_string(),
                value
                    .to_str()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|_| "<非文本响应头>".to_string()),
            )
        })
        .collect();
    let (mime_type, charset) = parse_content_type(&response_headers);
    let bytes_result = match &cancel {
        Some(token) => {
            tokio::select! {
                _ = token.notified() => {
                    return simple_failure(
                        def,
                        &url,
                        &resolved_headers,
                        body_preview,
                        tests,
                        logs,
                        script_error,
                        "请求已取消".to_string(),
                    );
                }
                result = response.bytes() => result,
            }
        }
        None => response.bytes().await,
    };
    let response_bytes = match bytes_result {
        Ok(bytes) => bytes,
        Err(err) => {
            return simple_failure(
                def,
                &url,
                &resolved_headers,
                body_preview,
                tests,
                logs,
                script_error,
                format!("读取响应体失败: {err}"),
            )
        }
    };
    let size = response_bytes.len();
    let (body_text, body_is_binary) = decode_body(&response_bytes, mime_type.as_deref());

    if !def.post_scripts.is_empty() {
        match load_scripts(&def.post_scripts, base_dir) {
            Ok(sources) => {
                let outcome = script::run_scripts(
                    &sources,
                    globals,
                    &request_vars,
                    &ScriptRequest {
                        method: def.method.clone(),
                        url: url.clone(),
                        body: body_preview.clone(),
                    },
                    Some(&ScriptResponse {
                        status,
                        headers: response_headers.clone(),
                        body_text: String::from_utf8_lossy(&response_bytes).to_string(),
                        mime_type: mime_type.clone(),
                        charset: charset.clone(),
                    }),
                );
                *globals = outcome.globals;
                tests.extend(outcome.tests);
                logs.extend(outcome.logs);
                if let Some(err) = outcome.error {
                    script_error = Some(err);
                }
            }
            Err(err) => script_error = Some(err),
        }
    }

    let saved_to = match &def.save_response {
        Some(spec) => match save_response(spec, &response_bytes, base_dir) {
            Ok(path) => Some(path.to_string_lossy().to_string()),
            Err(err) => {
                if script_error.is_none() {
                    script_error = Some(err);
                }
                None
            }
        },
        None => None,
    };

    RequestResult {
        request_index: def.index,
        name: def.display_name(),
        method: def.method.clone(),
        url,
        request_headers: resolved_headers
            .into_iter()
            .map(|(name, value)| KeyValue { name, value })
            .collect(),
        request_body: body_preview,
        status,
        status_text,
        response_headers: response_headers
            .into_iter()
            .map(|(name, value)| KeyValue { name, value })
            .collect(),
        body: body_text,
        body_is_binary,
        mime_type,
        size,
        duration_ms: elapsed,
        tests,
        logs,
        script_error,
        saved_to,
        error: None,
    }
}

fn build_body(
    def: &RequestDef,
    ctx: &VariableContext,
    base_dir: &Path,
) -> Result<Option<Vec<u8>>, String> {
    match &def.body {
        BodySpec::None => Ok(None),
        BodySpec::Inline(text) => {
            let resolved = variables::resolve(text, ctx);
            Ok(Some(expand_file_refs(&resolved, base_dir)?))
        }
        BodySpec::FromFile(path) => {
            let resolved = variables::resolve(path, ctx);
            let path = resolve_path(base_dir, &resolved);
            let bytes = std::fs::read(&path)
                .map_err(|e| format!("读取请求体文件失败 {}: {e}", path.display()))?;
            Ok(Some(bytes))
        }
    }
}

fn expand_file_refs(text: &str, base_dir: &Path) -> Result<Vec<u8>, String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let regex = RE.get_or_init(|| {
        Regex::new(r"(?m)^[ \t]*<[ \t]+([^<>\r\n]+?)[ \t]*\r?$").expect("valid regex")
    });
    if !regex.is_match(text) {
        return Ok(text.as_bytes().to_vec());
    }
    let mut out: Vec<u8> = Vec::new();
    let mut last = 0usize;
    for captures in regex.captures_iter(text) {
        let whole = captures.get(0).expect("match");
        out.extend_from_slice(text[last..whole.start()].as_bytes());
        let raw_path = captures.get(1).map(|m| m.as_str()).unwrap_or("").trim();
        let path = resolve_path(base_dir, raw_path);
        let bytes = std::fs::read(&path)
            .map_err(|e| format!("读取文件内容失败 {}: {e}", path.display()))?;
        out.extend_from_slice(&bytes);
        last = whole.end();
    }
    out.extend_from_slice(text[last..].as_bytes());
    Ok(out)
}

fn load_scripts(specs: &[ScriptSpec], base_dir: &Path) -> Result<Vec<String>, String> {
    let mut scripts = Vec::new();
    for spec in specs {
        match spec {
            ScriptSpec::Inline(source) => scripts.push(source.clone()),
            ScriptSpec::File(raw_path) => {
                let path = resolve_path(base_dir, raw_path);
                let source = std::fs::read_to_string(&path)
                    .map_err(|e| format!("读取脚本文件失败 {}: {e}", path.display()))?;
                scripts.push(source);
            }
        }
    }
    Ok(scripts)
}

fn decode_body(bytes: &[u8], mime: Option<&str>) -> (String, bool) {
    let text_like = mime
        .map(|m| {
            let m = m.to_ascii_lowercase();
            m.starts_with("text/")
                || m.contains("json")
                || m.contains("xml")
                || m.contains("javascript")
                || m.contains("x-www-form-urlencoded")
                || m.contains("svg")
                || m.contains("yaml")
                || m.contains("csv")
                || m.contains("graphql")
        })
        .unwrap_or(true);
    if text_like {
        match String::from_utf8(bytes.to_vec()) {
            Ok(text) => (text, false),
            Err(_) => (base64::engine::general_purpose::STANDARD.encode(bytes), true),
        }
    } else {
        (
            base64::engine::general_purpose::STANDARD.encode(bytes),
            true,
        )
    }
}

fn parse_content_type(headers: &[(String, String)]) -> (Option<String>, Option<String>) {
    let content_type = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.clone());
    let Some(content_type) = content_type else {
        return (None, None);
    };
    let mut parts = content_type.split(';');
    let mime = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let mut charset = None;
    for part in parts {
        let part = part.trim();
        if let Some(value) = part.strip_prefix("charset=") {
            charset = Some(
                value
                    .trim_matches('"')
                    .trim_matches('\'')
                    .trim()
                    .to_string(),
            );
        }
    }
    if mime.is_empty() {
        (None, charset)
    } else {
        (Some(mime), charset)
    }
}

fn save_response(spec: &SaveSpec, bytes: &[u8], base_dir: &Path) -> Result<PathBuf, String> {
    let path = resolve_path(base_dir, &spec.path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建响应保存目录失败: {e}"))?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).write(true);
    if spec.append {
        options.append(true);
    } else {
        options.truncate(true);
    }
    use std::io::Write;
    let mut file = options
        .open(&path)
        .map_err(|e| format!("打开响应保存文件失败 {}: {e}", path.display()))?;
    file.write_all(bytes)
        .map_err(|e| format!("写入响应保存文件失败: {e}"))?;
    Ok(path)
}

fn resolve_path(base_dir: &Path, raw: &str) -> PathBuf {
    let path = Path::new(raw.trim());
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base_dir.join(path)
    }
}

fn preview_bytes(bytes: &[u8]) -> String {
    match String::from_utf8(bytes.to_vec()) {
        Ok(text) => text,
        Err(_) => format!("<二进制请求体，{} 字节（Base64）: {}>", bytes.len(), base64::engine::general_purpose::STANDARD.encode(bytes)),
    }
}

#[allow(clippy::too_many_arguments)]
fn simple_failure(
    def: &RequestDef,
    url: &str,
    headers: &[(String, String)],
    body: Option<String>,
    tests: Vec<crate::models::TestResult>,
    logs: Vec<String>,
    script_error: Option<String>,
    message: String,
) -> RequestResult {
    RequestResult {
        request_index: def.index,
        name: def.display_name(),
        method: def.method.clone(),
        url: url.to_string(),
        request_headers: headers
            .iter()
            .map(|(name, value)| KeyValue {
                name: name.clone(),
                value: value.clone(),
            })
            .collect(),
        request_body: body,
        status: 0,
        status_text: String::new(),
        response_headers: Vec::new(),
        body: String::new(),
        body_is_binary: false,
        mime_type: None,
        size: 0,
        duration_ms: 0,
        tests,
        logs,
        script_error,
        saved_to: None,
        error: Some(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_text_and_binary() {
        let (text, binary) = decode_body(b"hello", Some("text/plain"));
        assert_eq!(text, "hello");
        assert!(!binary);
        let (body, binary) = decode_body(&[0xff, 0x00], Some("application/octet-stream"));
        assert!(binary);
        assert_eq!(body, "/wA=");
    }

    #[test]
    fn parses_content_type() {
        let headers = vec![(
            "Content-Type".to_string(),
            "application/json; charset=UTF-8".to_string(),
        )];
        let (mime, charset) = parse_content_type(&headers);
        assert_eq!(mime.as_deref(), Some("application/json"));
        assert_eq!(charset.as_deref(), Some("UTF-8"));
    }

    #[tokio::test]
    async fn executes_request_and_runs_post_script() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buffer = [0u8; 8192];
                    let _ = socket.read(&mut buffer).await;
                    let body = r#"{"id":42,"name":"test"}"#;
                    let response = format!(
                        "HTTP/1.1 201 Created\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nX-Test: yes\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });

        let script = r#"
client.test("status", function () {
  client.assert(response.status === 201, "bad status");
});
client.global.set("saved", response.body.name);
"#;
        let def = RequestDef {
            index: 0,
            name: Some("test".to_string()),
            method: "POST".to_string(),
            url: format!("http://{addr}/hello"),
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: BodySpec::Inline(r#"{"x":1}"#.to_string()),
            pre_scripts: Vec::new(),
            post_scripts: vec![ScriptSpec::Inline(script.to_string())],
            save_response: None,
            options: Default::default(),
            line: 1,
        };

        let mut globals = HashMap::new();
        let result = execute_request(
            &def,
            &HashMap::new(),
            &HashMap::new(),
            &mut globals,
            Arc::new(SharedCookieStore::new()),
            Path::new("."),
            None,
        )
        .await;

        assert_eq!(result.status, 201, "{:?}", result.error);
        assert!(result.body.contains("\"id\":42"));
        assert_eq!(result.response_headers.len(), 4);
        assert_eq!(result.tests.len(), 1);
        assert!(result.tests[0].passed);
        assert_eq!(result.mime_type.as_deref(), Some("application/json"));
        assert_eq!(globals.get("saved").map(String::as_str), Some("test"));
    }

    #[tokio::test]
    async fn cancels_inflight_request() {
        use tokio::net::TcpListener;

        // 服务端接受连接后保持沉默，模拟长时间无响应的请求
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    drop(socket);
                });
            }
        });

        let def = RequestDef {
            index: 0,
            name: None,
            method: "GET".to_string(),
            url: format!("http://{addr}/slow"),
            headers: Vec::new(),
            body: BodySpec::None,
            pre_scripts: Vec::new(),
            post_scripts: Vec::new(),
            save_response: None,
            options: Default::default(),
            line: 1,
        };

        let token = CancelToken::new();
        let canceller = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            canceller.cancel();
        });

        let mut globals = HashMap::new();
        let result = execute_request(
            &def,
            &HashMap::new(),
            &HashMap::new(),
            &mut globals,
            Arc::new(SharedCookieStore::new()),
            Path::new("."),
            Some(token),
        )
        .await;

        assert_eq!(result.status, 0);
        assert_eq!(result.error.as_deref(), Some("请求已取消"));
    }
}
