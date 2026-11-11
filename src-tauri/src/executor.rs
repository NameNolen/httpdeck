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

/// 请求体：普通字节流或结构化 multipart 表单。
enum PreparedBody {
    None,
    Bytes(Vec<u8>),
    Multipart(reqwest::multipart::Form),
}

pub async fn execute_request(
    def: &RequestDef,
    file_vars: &HashMap<String, String>,
    env_vars: &HashMap<String, String>,
    globals: &mut HashMap<String, String>,
    cookies: Arc<SharedCookieStore>,
    base_dir: &Path,
    cancel: Option<Arc<CancelToken>>,
    proxy: Option<&str>,
    identity: Option<reqwest::Identity>,
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
                        timeout_ms: def.options.timeout_ms,
                        no_redirect: def.options.no_redirect,
                        redirected: false,
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
                    timeout_ms: def.options.timeout_ms,
                    no_redirect: def.options.no_redirect,
                    redirected: false,
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
    let mut resolved_headers: Vec<(String, String)> = def
        .headers
        .iter()
        .map(|(n, v)| (n.clone(), variables::resolve(v, &ctx)))
        .collect();
    // GraphQL 块自动补 Content-Type
    if def.graphql
        && !resolved_headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("content-type"))
    {
        resolved_headers.push(("Content-Type".to_string(), "application/json".to_string()));
    }
    let multipart_content_type = resolved_headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.trim().to_string())
        .filter(|value| value.to_ascii_lowercase().starts_with("multipart/"));
    let body_result = build_body(def, &ctx, base_dir, multipart_content_type.as_deref());
    let (prepared_body, body_preview) = match body_result {
        Ok(prepared) => prepared,
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
    let is_multipart = matches!(prepared_body, PreparedBody::Multipart(_));

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
        // multipart 由 reqwest 生成新的 boundary，丢弃用户手写的 Content-Type
        if is_multipart && name.eq_ignore_ascii_case("content-type") {
            continue;
        }
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
    if let Some(identity) = identity {
        client_builder = client_builder.identity(identity);
    }
    if let Some(proxy) = proxy.filter(|p| !p.trim().is_empty()) {
        match reqwest::Proxy::all(proxy.trim()) {
            Ok(proxy) => client_builder = client_builder.proxy(proxy),
            Err(err) => {
                return simple_failure(
                    def,
                    &url,
                    &resolved_headers,
                    body_preview,
                    tests,
                    logs,
                    script_error,
                    format!("代理地址无效 `{proxy}`: {err}"),
                )
            }
        }
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

    let mut request = client
        .request(method.clone(), parsed_url.clone())
        .headers(header_map);
    match prepared_body {
        PreparedBody::None => {}
        PreparedBody::Bytes(bytes) => request = request.body(bytes),
        PreparedBody::Multipart(form) => request = request.multipart(form),
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
    let redirected = response.url() != &parsed_url;
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
    let (body_text, body_is_binary) =
        decode_body(&response_bytes, mime_type.as_deref(), charset.as_deref());

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
                        // 脚本拿到的是按 charset 解码后的文本；二进制响应退回宽松 UTF-8
                        body_text: if body_is_binary {
                            String::from_utf8_lossy(&response_bytes).to_string()
                        } else {
                            body_text.clone()
                        },
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
        timeout_ms: def.options.timeout_ms,
        no_redirect: def.options.no_redirect,
        redirected,
        error: None,
    }
}

fn build_body(
    def: &RequestDef,
    ctx: &VariableContext,
    base_dir: &Path,
    multipart_content_type: Option<&str>,
) -> Result<(PreparedBody, Option<String>), String> {
    match &def.body {
        BodySpec::None => Ok((PreparedBody::None, None)),
        BodySpec::Inline(text) => {
            let resolved = variables::resolve(text, ctx);
            if let Some(content_type) = multipart_content_type {
                // 结构化 multipart 解析失败时退回原始字节替换，保证兼容性
                if let Ok(form) = build_multipart_form(&resolved, content_type, base_dir) {
                    return Ok((PreparedBody::Multipart(form), Some(resolved)));
                }
            }
            let bytes = expand_file_refs(&resolved, base_dir)?;
            Ok((PreparedBody::Bytes(bytes), Some(resolved)))
        }
        BodySpec::FromFile(path) => {
            let resolved = variables::resolve(path, ctx);
            let path = resolve_path(base_dir, &resolved);
            let bytes = std::fs::read(&path)
                .map_err(|e| format!("读取请求体文件失败 {}: {e}", path.display()))?;
            let preview = preview_bytes(&bytes);
            Ok((PreparedBody::Bytes(bytes), Some(preview)))
        }
    }
}

/// 一个 multipart 分段的原始形态（尚未转换成分段内容）。
struct RawPart {
    headers: Vec<(String, String)>,
    content: String,
}

/// 把 `.http` 中的 multipart 原始文本解析为 `reqwest::multipart::Form`。
///
/// 支持 IntelliJ 的文件引用语法：分段内容只有一行 `< ./file.bin` 时读取文件。
fn build_multipart_form(
    text: &str,
    content_type: &str,
    base_dir: &Path,
) -> Result<reqwest::multipart::Form, String> {
    let boundary = header_param(content_type, "boundary")
        .or_else(|| detect_boundary(text))
        .ok_or_else(|| "multipart 请求缺少 boundary".to_string())?;
    let parts = split_multipart(text, &boundary);
    if parts.is_empty() {
        return Err("multipart 请求体中没有可解析的分段".to_string());
    }
    let mut form = reqwest::multipart::Form::new();
    for part in parts {
        let disposition = part
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-disposition"))
            .map(|(_, value)| value.as_str())
            .unwrap_or("");
        let Some(field_name) = header_param(disposition, "name").filter(|n| !n.is_empty()) else {
            continue;
        };
        let filename = header_param(disposition, "filename").filter(|n| !n.is_empty());
        let part_mime = part
            .headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            .map(|(_, value)| value.trim().to_string())
            .filter(|value| !value.is_empty());

        let file_ref = part_content_file_ref(&part.content);
        let mut piece = match &file_ref {
            Some(raw_path) => {
                let path = resolve_path(base_dir, raw_path);
                let bytes = std::fs::read(&path)
                    .map_err(|e| format!("读取文件内容失败 {}: {e}", path.display()))?;
                reqwest::multipart::Part::bytes(bytes)
            }
            None => reqwest::multipart::Part::text(part.content),
        };
        if let Some(filename) = filename {
            piece = piece.file_name(filename);
        }
        if let Some(mime) = part_mime {
            piece = piece
                .mime_str(&mime)
                .map_err(|e| format!("multipart 分段 Content-Type 无效 `{mime}`: {e}"))?;
        }
        form = form.part(field_name, piece);
    }
    Ok(form)
}

/// 从 `Content-Type` 一类字符串中提取 `key=value` 参数（忽略大小写）。
fn header_param(raw: &str, key: &str) -> Option<String> {
    for segment in raw.split(';') {
        let segment = segment.trim();
        let Some((name, value)) = segment.split_once('=') else {
            continue;
        };
        if name.trim().eq_ignore_ascii_case(key) {
            let value = value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            return Some(value);
        }
    }
    None
}

/// 从请求体首行推断 boundary（兼容用户未写 Content-Type 的情况）。
fn detect_boundary(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("--") {
            let boundary = rest.trim_end_matches("--").trim();
            if !boundary.is_empty() {
                return Some(boundary.to_string());
            }
        }
        if !trimmed.is_empty() {
            return None;
        }
    }
    None
}

/// 按 boundary 切分 multipart 文本，收集每段的分段头和内容。
fn split_multipart(text: &str, boundary: &str) -> Vec<RawPart> {
    let delimiter = format!("--{boundary}");
    let closing = format!("{delimiter}--");
    let lines: Vec<&str> = text.split('\n').collect();
    let mut parts = Vec::new();
    let mut index = 0usize;

    // 跳过分隔符前的内容（preamble）
    while index < lines.len() && lines[index].trim_end_matches('\r').trim() != delimiter {
        index += 1;
    }
    index += 1;

    while index < lines.len() {
        let current = lines[index].trim_end_matches('\r').trim();
        if current == closing {
            break;
        }
        if current == delimiter {
            index += 1;
            continue;
        }

        let mut headers = Vec::new();
        let mut terminated = false;
        while index < lines.len() {
            let line = lines[index].trim_end_matches('\r');
            index += 1;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                terminated = true;
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                headers.push((name.trim().to_string(), value.trim().to_string()));
            }
        }
        if !terminated {
            break;
        }

        let mut content_lines = Vec::new();
        while index < lines.len() {
            let line = lines[index].trim_end_matches('\r');
            let trimmed = line.trim();
            if trimmed == delimiter || trimmed == closing {
                break;
            }
            content_lines.push(line);
            index += 1;
        }
        // 分段内容与下一个 boundary 之间的换行由 split 保留在内容里
        parts.push(RawPart {
            headers,
            content: content_lines.join("\n"),
        });
    }

    parts
}

/// 分段内容为单行 `< ./file` 时返回文件路径。
fn part_content_file_ref(content: &str) -> Option<String> {
    let trimmed = content.trim();
    let rest = trimmed.strip_prefix('<')?.trim();
    if rest.is_empty() || trimmed.contains('\n') {
        return None;
    }
    Some(rest.to_string())
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

fn decode_body(
    bytes: &[u8],
    mime: Option<&str>,
    charset: Option<&str>,
) -> (String, bool) {
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
    if !text_like {
        return (
            base64::engine::general_purpose::STANDARD.encode(bytes),
            true,
        );
    }
    // 1. 优先按 Content-Type 声明的 charset 解码
    if let Some(label) = charset.map(str::trim).filter(|label| !label.is_empty()) {
        if let Some(encoding) = encoding_rs::Encoding::for_label(label.as_bytes()) {
            let (text, _, _) = encoding.decode(bytes);
            return (text.into_owned(), false);
        }
    }
    // 2. 没有可用 charset 时按 BOM 判断（UTF-8 / UTF-16）
    if let Some((encoding, bom_len)) = encoding_rs::Encoding::for_bom(bytes) {
        let (text, _) = encoding.decode_without_bom_handling(&bytes[bom_len..]);
        return (text.into_owned(), false);
    }
    // 3. 最后尝试严格 UTF-8，非法字节仍按二进制处理
    match String::from_utf8(bytes.to_vec()) {
        Ok(text) => (text, false),
        Err(_) => (
            base64::engine::general_purpose::STANDARD.encode(bytes),
            true,
        ),
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
        timeout_ms: def.options.timeout_ms,
        no_redirect: def.options.no_redirect,
        redirected: false,
        error: Some(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_text_and_binary() {
        let (text, binary) = decode_body(b"hello", Some("text/plain"), None);
        assert_eq!(text, "hello");
        assert!(!binary);
        let (body, binary) = decode_body(&[0xff, 0x00], Some("application/octet-stream"), None);
        assert!(binary);
        assert_eq!(body, "/wA=");
        // 文本类型但非法 UTF-8、未声明 charset 时仍按二进制处理
        let (body, binary) = decode_body(&[0xff, 0x00], Some("text/plain"), None);
        assert!(binary);
        assert_eq!(body, "/wA=");
    }

    #[test]
    fn decodes_declared_charset_and_bom() {
        // GBK 编码的“中文”
        let (text, binary) = decode_body(&[0xd6, 0xd0, 0xce, 0xc4], Some("text/plain"), Some("gbk"));
        assert_eq!(text, "中文");
        assert!(!binary);

        // UTF-16LE BOM + "hi"
        let mut utf16 = vec![0xff, 0xfe];
        utf16.extend_from_slice(&[b'h', 0, b'i', 0]);
        let (text, binary) = decode_body(&utf16, Some("text/plain"), None);
        assert_eq!(text, "hi");
        assert!(!binary);
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
            graphql: false,
            name: Some("test".to_string()),
            method: "POST".to_string(),
            url: format!("http://{addr}/hello"),
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: BodySpec::Inline(r#"{"x":1}"#.to_string()),
            pre_scripts: Vec::new(),
            post_scripts: vec![ScriptSpec::Inline(script.to_string())],
            save_response: None,
            options: crate::models::RequestOptions {
                timeout_ms: Some(1234),
                no_redirect: true,
                ..Default::default()
            },
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
            None,
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
        assert_eq!(result.timeout_ms, Some(1234));
        assert!(result.no_redirect);
        assert!(!result.redirected);
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
            graphql: false,
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
            None,
            None,
        )
        .await;

        assert_eq!(result.status, 0);
        assert_eq!(result.error.as_deref(), Some("请求已取消"));
    }

    #[tokio::test]
    async fn proxy_is_applied() {
        use tokio::net::TcpListener;

        // 目标服务器本身可用
        let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = server.accept().await {
                tokio::spawn(async move {
                    // 先读完请求再响应，避免连接被提前重置
                    let mut buffer = [0u8; 1024];
                    let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buffer).await;
                    let response = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
                    let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, response.as_bytes()).await;
                });
            }
        });

        // 先占后放一个端口，得到几乎确定未监听的代理端口
        let free_port = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap()
            .port();

        let def = RequestDef {
            index: 0,
            graphql: false,
            name: None,
            method: "GET".to_string(),
            url: format!("http://{addr}/"),
            headers: Vec::new(),
            body: BodySpec::None,
            pre_scripts: Vec::new(),
            post_scripts: Vec::new(),
            save_response: None,
            options: Default::default(),
            line: 1,
        };

        // 无代理时成功
        let mut globals = HashMap::new();
        let ok = execute_request(
            &def,
            &HashMap::new(),
            &HashMap::new(),
            &mut globals,
            Arc::new(SharedCookieStore::new()),
            Path::new("."),
            None,
            None,
            None,
        )
        .await;
        assert_eq!(ok.status, 200, "{:?}", ok.error);

        // 指向未监听端口的代理应失败，证明代理生效
        let failed = execute_request(
            &def,
            &HashMap::new(),
            &HashMap::new(),
            &mut globals,
            Arc::new(SharedCookieStore::new()),
            Path::new("."),
            None,
            Some(&format!("http://127.0.0.1:{free_port}")),
            None,
        )
        .await;
        assert_eq!(failed.status, 0);
        assert!(failed.error.is_some());
    }

    #[tokio::test]
    async fn reports_redirect_state() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            for _ in 0..2 {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = [0u8; 2048];
                let _ = socket.read(&mut buf).await;
                let request = String::from_utf8_lossy(&buf);
                let response = if request.starts_with("GET /final") {
                    "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok"
                } else {
                    "HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                };
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });

        let def = RequestDef {
            index: 0,
            graphql: false,
            name: None,
            method: "GET".to_string(),
            url: format!("http://{addr}/start"),
            headers: Vec::new(),
            body: BodySpec::None,
            pre_scripts: Vec::new(),
            post_scripts: Vec::new(),
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
            None,
            None,
        )
        .await;

        assert_eq!(result.status, 200, "{:?}", result.error);
        assert!(result.redirected);
        assert!(!result.no_redirect);
        assert_eq!(result.timeout_ms, None);
    }

    #[tokio::test]
    async fn sends_structured_multipart_with_file_ref() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let dir = std::env::temp_dir().join(format!(
            "httpdeck-multipart-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("note.txt"), "file-payload").unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut buf: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let read = socket.read(&mut chunk).await.unwrap_or(0);
                if read == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..read]);
                if let Some(header_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..header_end]).to_ascii_lowercase();
                    let length = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if buf.len() >= header_end + 4 + length {
                        break;
                    }
                }
            }
            let _ = tx.send(String::from_utf8_lossy(&buf).to_string());
            let response =
                "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        });

        let body = "--WebAppBoundary\n\
Content-Disposition: form-data; name=\"title\"\n\
\n\
hello\n\
--WebAppBoundary\n\
Content-Disposition: form-data; name=\"file\"; filename=\"note.txt\"\n\
Content-Type: text/plain\n\
\n\
< ./note.txt\n\
--WebAppBoundary--";
        let def = RequestDef {
            index: 0,
            graphql: false,
            name: None,
            method: "POST".to_string(),
            url: format!("http://{addr}/upload"),
            headers: vec![(
                "Content-Type".to_string(),
                "multipart/form-data; boundary=WebAppBoundary".to_string(),
            )],
            body: BodySpec::Inline(body.to_string()),
            pre_scripts: Vec::new(),
            post_scripts: Vec::new(),
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
            &dir,
            None,
            None,
            None,
        )
        .await;
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(result.status, 200, "{:?}", result.error);
        let captured = rx.await.expect("captured request");
        let lowercase = captured.to_ascii_lowercase();
        assert!(lowercase.contains("content-type: multipart/form-data; boundary="));
        assert!(!lowercase.contains("boundary=webappboundary"));
        assert!(captured.contains("name=\"title\""));
        assert!(captured.contains("hello"));
        assert!(captured.contains("name=\"file\""));
        assert!(captured.contains("filename=\"note.txt\""));
        assert!(captured.contains("file-payload"));
    }
}
