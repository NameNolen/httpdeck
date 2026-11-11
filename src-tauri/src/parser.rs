use crate::models::{
    KeyValue, ParsedFileOutput, RequestOptions, RequestSummary,
};
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub enum ScriptSpec {
    Inline(String),
    File(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum BodySpec {
    None,
    Inline(String),
    FromFile(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SaveSpec {
    pub path: String,
    pub append: bool,
}

#[derive(Debug, Clone)]
pub struct RequestDef {
    pub index: usize,
    pub name: Option<String>,
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: BodySpec,
    pub pre_scripts: Vec<ScriptSpec>,
    pub post_scripts: Vec<ScriptSpec>,
    pub save_response: Option<SaveSpec>,
    pub options: RequestOptions,
    /// 请求体来自 `graphql { ... }` 块，已转换为 JSON（{"query": ...}）
    pub graphql: bool,
    pub line: usize,
}

impl RequestDef {
    pub fn display_name(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| auto_name(&self.method, &self.url))
    }

    pub fn body_kind(&self) -> &'static str {
        match self.body {
            BodySpec::None => "none",
            BodySpec::Inline(_) => "inline",
            BodySpec::FromFile(_) => "file",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ParsedHttpFile {
    pub requests: Vec<RequestDef>,
    pub variables: Vec<(String, String)>,
    pub issues: Vec<String>,
}

impl ParsedHttpFile {
    pub fn to_output(&self) -> ParsedFileOutput {
        ParsedFileOutput {
            requests: self
                .requests
                .iter()
                .map(|r| RequestSummary {
                    index: r.index,
                    name: r.display_name(),
                    method: r.method.clone(),
                    url: r.url.clone(),
                    line: r.line,
                    has_pre_script: !r.pre_scripts.is_empty(),
                    has_post_script: !r.post_scripts.is_empty(),
                    body_kind: r.body_kind().to_string(),
                })
                .collect(),
            variables: self
                .variables
                .iter()
                .map(|(k, v)| KeyValue {
                    name: k.clone(),
                    value: v.clone(),
                })
                .collect(),
            issues: self.issues.clone(),
        }
    }
}

const METHODS: &[&str] = &[
    "GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "TRACE", "CONNECT",
];

pub fn parse(content: &str) -> ParsedHttpFile {
    let lines: Vec<&str> = content.lines().collect();
    let mut result = ParsedHttpFile::default();
    let mut pending_pre: Vec<ScriptSpec> = Vec::new();
    let mut pending_name: Option<String> = None;
    let mut section_label: Option<String> = None;
    let mut i = 0usize;

    while i < lines.len() {
        let raw = lines[i];
        let trimmed = raw.trim();

        if trimmed.starts_with("###") {
            pending_name = None;
            let label = trimmed.trim_start_matches('#').trim();
            section_label = if label.is_empty() {
                None
            } else {
                Some(label.to_string())
            };
            i += 1;
            continue;
        }

        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        if trimmed.starts_with("//") {
            if let Some(name) = parse_name_directive(trimmed) {
                pending_name = Some(name);
            }
            i += 1;
            continue;
        }

        if trimmed.starts_with('#') {
            i += 1;
            continue;
        }

        if trimmed.starts_with('@') {
            match parse_var_line(trimmed) {
                Some((k, v)) => result.variables.push((k, v)),
                None => result
                    .issues
                    .push(format!("第 {} 行：无法解析变量定义", i + 1)),
            }
            i += 1;
            continue;
        }

        if trimmed.starts_with('>') {
            match read_directive(&lines, i) {
                Some((Directive::Script(spec), next)) => {
                    pending_pre.push(spec);
                    i = next;
                    continue;
                }
                Some((Directive::Save(_), next)) => {
                    result
                        .issues
                        .push(format!("第 {} 行：响应保存指令必须位于请求之后", i + 1));
                    i = next;
                    continue;
                }
                None => {
                    result
                        .issues
                        .push(format!("第 {} 行：无法解析脚本或响应指令", i + 1));
                    i += 1;
                    continue;
                }
            }
        }

        if let Some((method, url)) = parse_request_line(raw) {
            let mut builder = RequestBuilder::new(
                method,
                url,
                i + 1,
                pending_name.take(),
                section_label.clone(),
                std::mem::take(&mut pending_pre),
            );
            i += 1;
            i = parse_request_body(&lines, i, &mut builder, &mut result.issues);
            result.requests.push(builder.build());
        } else {
            i += 1;
        }
    }

    for (idx, req) in result.requests.iter_mut().enumerate() {
        req.index = idx;
    }
    result
}

struct RequestBuilder {
    name_override: Option<String>,
    section_label: Option<String>,
    method: String,
    url: String,
    headers: Vec<(String, String)>,
    body: BodySpec,
    pre_scripts: Vec<ScriptSpec>,
    post_scripts: Vec<ScriptSpec>,
    save_response: Option<SaveSpec>,
    options: RequestOptions,
    graphql: bool,
    line: usize,
}

impl RequestBuilder {
    fn new(
        method: String,
        url: String,
        line: usize,
        name_override: Option<String>,
        section_label: Option<String>,
        pre_scripts: Vec<ScriptSpec>,
    ) -> Self {
        Self {
            name_override,
            section_label,
            method,
            url,
            headers: Vec::new(),
            body: BodySpec::None,
            pre_scripts,
            post_scripts: Vec::new(),
            save_response: None,
            options: RequestOptions::default(),
            graphql: false,
            line,
        }
    }

    fn build(self) -> RequestDef {
        let name = self
            .name_override
            .or(self.section_label)
            .filter(|s| !s.trim().is_empty());
        RequestDef {
            index: 0,
            name,
            method: self.method,
            url: self.url,
            headers: self.headers,
            body: self.body,
            pre_scripts: self.pre_scripts,
            post_scripts: self.post_scripts,
            save_response: self.save_response,
            options: self.options,
            graphql: self.graphql,
            line: self.line,
        }
    }
}

enum Directive {
    Script(ScriptSpec),
    Save(SaveSpec),
}

fn read_directive(lines: &[&str], start: usize) -> Option<(Directive, usize)> {
    let raw = lines[start];
    let trimmed = raw.trim();

    if let Some(path) = trimmed.strip_prefix(">>!") {
        let path = path.trim();
        if path.is_empty() {
            return None;
        }
        return Some((
            Directive::Save(SaveSpec {
                path: path.to_string(),
                append: false,
            }),
            start + 1,
        ));
    }

    if let Some(path) = trimmed.strip_prefix(">>") {
        let path = path.trim();
        if path.is_empty() {
            return None;
        }
        return Some((
            Directive::Save(SaveSpec {
                path: path.to_string(),
                append: true,
            }),
            start + 1,
        ));
    }

    if let Some(path) = trimmed.strip_prefix("<>") {
        let path = path.trim();
        if path.is_empty() {
            return None;
        }
        return Some((
            Directive::Save(SaveSpec {
                path: path.to_string(),
                append: false,
            }),
            start + 1,
        ));
    }

    if !trimmed.starts_with('>') {
        return None;
    }

    let rest = trimmed[1..].trim();
    if let Some(script) = rest.strip_prefix("{%") {
        let mut script = script.to_string();
        if let Some(pos) = script.find("%}") {
            return Some((
                Directive::Script(ScriptSpec::Inline(
                    script[..pos].trim().to_string(),
                )),
                start + 1,
            ));
        }
        let mut i = start + 1;
        while i < lines.len() {
            let line = lines[i];
            if let Some(pos) = line.find("%}") {
                script.push('\n');
                script.push_str(&line[..pos]);
                i += 1;
                break;
            }
            script.push('\n');
            script.push_str(line);
            i += 1;
        }
        return Some((
            Directive::Script(ScriptSpec::Inline(script.trim().to_string())),
            i,
        ));
    }

    if !rest.is_empty() {
        return Some((
            Directive::Script(ScriptSpec::File(rest.to_string())),
            start + 1,
        ));
    }

    None
}

/// 识别 IntelliJ 风格的 GraphQL 请求体（`graphql { ... }` 或 `graphql` 后跟查询文本），
/// 转换为 JSON 内联体 `{"query": ..., "variables": ...}`。
/// 无法识别时保持 BodySpec::None，由调用方回退为普通文本体。
fn try_parse_graphql_body(body_lines: &[&str], builder: &mut RequestBuilder) {
    let Some(first_idx) = body_lines.iter().position(|l| !l.trim().is_empty()) else {
        return;
    };
    let first = body_lines[first_idx].trim();
    let Some(rest) = first.strip_prefix("graphql") else {
        return;
    };
    // 必须是独立关键字：后面只能是空白或 '{'（避免误匹配 graphqlFoo 之类）
    if let Some(c) = rest.chars().next() {
        if c != '{' && !c.is_whitespace() {
            return;
        }
    }
    let remainder = rest.trim_start();
    let braced = remainder.starts_with('{');
    let mut content: Vec<String> = Vec::new();
    if braced {
        let inner = remainder[1..].trim_end();
        if !inner.is_empty() {
            content.push(inner.to_string());
        }
    } else if !remainder.is_empty() {
        content.push(remainder.to_string());
    }
    for line in &body_lines[first_idx + 1..] {
        content.push(line.to_string());
    }
    // 花括号包裹时去掉最外层的收尾 '}'
    if braced {
        strip_trailing_brace(&mut content);
    }
    match graphql_content_to_json(&content) {
        Some(json) => {
            builder.body = BodySpec::Inline(json);
            builder.graphql = true;
        }
        None => {}
    }
}

fn graphql_content_to_json(content: &[String]) -> Option<String> {
    let mut query_lines: Vec<String> = Vec::new();
    let mut var_lines: Vec<String> = Vec::new();
    let mut in_vars = false;
    for line in content {
        let t = line.trim();
        if in_vars {
            var_lines.push(line.clone());
            continue;
        }
        if t == "variables {" || t == "variables{" {
            in_vars = true;
            continue;
        }
        if let Some(after) = t.strip_prefix("variables{") {
            in_vars = true;
            var_lines.push(after.to_string());
            continue;
        }
        if let Some(after) = t.strip_prefix("variables {") {
            in_vars = true;
            let after = after.trim();
            if !after.is_empty() {
                var_lines.push(after.to_string());
            }
            continue;
        }
        query_lines.push(line.clone());
    }
    let query = dedent_lines(&query_lines);
    if query.is_empty() {
        return None;
    }
    let mut payload = serde_json::json!({ "query": query });
    if in_vars {
        strip_trailing_brace(&mut var_lines);
        let vars_text = dedent_lines(&var_lines);
        if vars_text.is_empty() {
            return None;
        }
        let wrapped = format!("{{ {vars_text} }}");
        match serde_json::from_str::<serde_json::Value>(&wrapped) {
            Ok(v) if v.is_object() => payload["variables"] = v,
            _ => return None,
        }
    }
    serde_json::to_string(&payload).ok()
}

/// 去掉最后一个非空行末尾的一个 '}'
fn strip_trailing_brace(lines: &mut Vec<String>) {
    if let Some(last) = lines.iter_mut().rfind(|l| !l.trim().is_empty()) {
        let t = last.trim_end();
        if t.ends_with('}') {
            *last = t[..t.len() - 1].trim_end().to_string();
        }
    }
}

/// 按公共缩进去缩进，跳过空行，返回 trim 后的文本
fn dedent_lines(lines: &[String]) -> String {
    let indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let mut out = String::new();
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let own_indent = line.len() - line.trim_start().len();
        out.push_str(&line[own_indent.min(indent)..]);
        out.push('\n');
    }
    out.trim().to_string()
}

fn parse_request_body(
    lines: &[&str],
    mut i: usize,
    builder: &mut RequestBuilder,
    issues: &mut Vec<String>,
) -> usize {
    // 多行 URL 续行（缩进且不是 header）
    while i < lines.len() {
        let raw = lines[i];
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            break;
        }
        let indented = raw.starts_with(' ') || raw.starts_with('\t');
        if !indented {
            break;
        }
        if trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with('>') {
            break;
        }
        if split_header(trimmed).is_some() {
            break;
        }
        builder.url.push_str(trimmed);
        i += 1;
    }

    // header 区
    let mut body_started = false;
    while i < lines.len() {
        let raw = lines[i];
        let trimmed = raw.trim();
        if trimmed.starts_with("###") {
            return i;
        }
        if trimmed.is_empty() {
            body_started = true;
            i += 1;
            break;
        }
        if trimmed.starts_with('>') {
            return i;
        }
        if trimmed.starts_with("//") {
            parse_option_directive(trimmed, builder);
            i += 1;
            continue;
        }
        if trimmed.starts_with('#') {
            i += 1;
            continue;
        }
        if let Some((name, value)) = split_header(trimmed) {
            builder.headers.push((name, value));
            i += 1;
            continue;
        }
        body_started = true;
        break;
    }

    // body 区
    if body_started {
        let mut body_lines: Vec<&str> = Vec::new();
        while i < lines.len() {
            let raw = lines[i];
            let trimmed = raw.trim();
            if trimmed.starts_with("###") || trimmed.starts_with('>') {
                break;
            }
            body_lines.push(raw);
            i += 1;
        }
        while body_lines
            .last()
            .map(|l| l.trim().is_empty())
            .unwrap_or(false)
        {
            body_lines.pop();
        }
        if !body_lines.is_empty() {
            let first = body_lines[0].trim_start();
            if body_lines.len() == 1
                && first.starts_with("< ")
                && !first.starts_with("<>")
            {
                builder.body = BodySpec::FromFile(first[2..].trim().to_string());
            } else {
                try_parse_graphql_body(&body_lines, builder);
                if builder.body == BodySpec::None {
                    builder.body = BodySpec::Inline(body_lines.join("\n"));
                }
            }
        }
    }

    // 响应处理脚本 / 保存指令
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed.starts_with("###") {
            break;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') && !trimmed.starts_with("#!") {
            i += 1;
            continue;
        }
        if trimmed.starts_with("//") {
            i += 1;
            continue;
        }
        if trimmed.starts_with('>') || trimmed.starts_with("<>") {
            if let Some((directive, next)) = read_directive(lines, i) {
                match directive {
                    Directive::Script(spec) => builder.post_scripts.push(spec),
                    Directive::Save(spec) => builder.save_response = Some(spec),
                }
                i = next;
                continue;
            } else {
                issues.push(format!("第 {} 行：无法解析响应指令", i + 1));
            }
        }
        i += 1;
    }

    i
}

fn parse_option_directive(line: &str, builder: &mut RequestBuilder) {
    let body = line.trim_start_matches("//").trim();
    let mut parts = body.split_whitespace();
    let Some(key) = parts.next() else {
        return;
    };
    if !key.starts_with('@') {
        return;
    }
    match key.to_ascii_lowercase().as_str() {
        "@no-redirect" => builder.options.no_redirect = true,
        "@no-cookie-jar" => builder.options.no_cookie_jar = true,
        "@no-log" => builder.options.no_log = true,
        "@no-ssl-verification" => builder.options.accept_invalid_certs = true,
        "@websocket" => builder.options.websocket = true,
        "@timeout" => {
            if let Some(value) = parts.next() {
                builder.options.timeout_ms = value.parse::<u64>().ok();
            }
        }
        "@name" => {
            if let Some(value) = parts.next() {
                builder.name_override = Some(value.to_string());
            }
        }
        _ => {}
    }
}

fn parse_name_directive(line: &str) -> Option<String> {
    let body = line.trim_start_matches("//").trim();
    let value = body.strip_prefix("@name")?.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn parse_var_line(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix('@')?;
    let (name, value) = rest.split_once('=')?;
    let name = name.trim();
    if name.is_empty() || name.chars().any(char::is_whitespace) {
        return None;
    }
    let mut value = value.trim().to_string();
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
        {
            value = value[1..value.len() - 1].to_string();
        }
    }
    Some((name.to_string(), value))
}

fn split_header(line: &str) -> Option<(String, String)> {
    let (name, value) = line.split_once(':')?;
    let name = name.trim();
    if name.is_empty() || name.chars().any(char::is_whitespace) {
        return None;
    }
    if name.starts_with("//") || name.starts_with('#') {
        return None;
    }
    Some((name.to_string(), value.trim().to_string()))
}

fn parse_request_line(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with('#') || trimmed.starts_with('@') {
        return None;
    }
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let method = parts.next()?.to_ascii_uppercase();
    if !METHODS.contains(&method.as_str()) {
        return None;
    }
    let url = parts.next().unwrap_or("").trim().to_string();
    if url.is_empty() {
        return None;
    }
    Some((method, url))
}

fn auto_name(method: &str, url: &str) -> String {
    if let Ok(parsed) = Url::parse(url) {
        let path = if parsed.path().is_empty() {
            "/"
        } else {
            parsed.path()
        };
        return format!("{} {}", method, path);
    }
    format!("{} {}", method, url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_file() {
        let src = r#"
@host = https://api.example.com

### List users
# a comment
// @name listUsers
GET {{host}}/api/users?page=1
Authorization: Bearer {{token}}
Accept: application/json

> {%
    client.test("ok", function() {
        client.assert(response.status === 200, "bad status");
    });
%}

### Create user
POST {{host}}/api/users
Content-Type: application/json

{
  "name": "Alice"
}
"#;
        let parsed = parse(src);
        assert_eq!(parsed.requests.len(), 2);
        assert_eq!(parsed.variables.len(), 1);
        assert_eq!(parsed.requests[0].name.as_deref(), Some("listUsers"));
        assert_eq!(parsed.requests[0].method, "GET");
        assert_eq!(parsed.requests[0].headers.len(), 2);
        assert_eq!(parsed.requests[0].post_scripts.len(), 1);
        assert_eq!(parsed.requests[1].method, "POST");
        assert_eq!(
            parsed.requests[1].body,
            BodySpec::Inline("{\n  \"name\": \"Alice\"\n}".to_string())
        );
    }

    #[test]
    fn parses_pre_script_and_file_body() {
        let src = r#"
> ./scripts/pre.js

### upload
POST http://localhost:8080/upload
Content-Type: application/octet-stream

< ./payload.bin
>>! ./out.bin
"#;
        let parsed = parse(src);
        assert_eq!(parsed.requests.len(), 1);
        assert_eq!(
            parsed.requests[0].pre_scripts,
            vec![ScriptSpec::File("./scripts/pre.js".to_string())]
        );
        assert_eq!(
            parsed.requests[0].body,
            BodySpec::FromFile("./payload.bin".to_string())
        );
        assert_eq!(
            parsed.requests[0].save_response,
            Some(SaveSpec {
                path: "./out.bin".to_string(),
                append: false
            })
        );
    }

    #[test]
    fn parses_inline_pre_script_and_multiline_url() {
        let src = r#"
> {% client.global.set("token", "abc"); %}

### request
GET http://localhost:8080/api/
    users?limit=10
X-Test: yes
"#;
        let parsed = parse(src);
        assert_eq!(parsed.requests.len(), 1);
        assert_eq!(parsed.requests[0].url, "http://localhost:8080/api/users?limit=10");
        assert_eq!(parsed.requests[0].pre_scripts.len(), 1);
        assert_eq!(parsed.requests[0].headers.len(), 1);
    }

    #[test]
    fn parses_options() {
        let src = r#"
### options
GET http://localhost:8080/no-redirect
// @no-redirect
// @timeout 3500
// @no-cookie-jar
"#;
        let parsed = parse(src);
        assert!(parsed.requests[0].options.no_redirect);
        assert!(parsed.requests[0].options.no_cookie_jar);
        assert_eq!(parsed.requests[0].options.timeout_ms, Some(3500));
    }

    #[test]
    fn parses_graphql_braced_body_with_variables() {
        let src = r#"
### GraphQL request
POST https://api.github.com/graphql
Content-Type: application/json

graphql {
  query {
    viewer {
      login
    }
  }
  variables {
    "first": 10
  }
}
"#;
        let parsed = parse(src);
        assert!(parsed.requests[0].graphql);
        let BodySpec::Inline(body) = &parsed.requests[0].body else {
            panic!("expected inline body");
        };
        let value: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(
            value["query"].as_str().unwrap(),
            "query {\n  viewer {\n    login\n  }\n}"
        );
        assert_eq!(value["variables"]["first"], 10);
        assert!(value.get("query").is_some());
    }

    #[test]
    fn parses_graphql_unbraced_body() {
        let src = r#"
### GraphQL no variables
POST https://api.example.com/graphql

graphql
query GetUser($id: ID!) {
  user(id: $id) {
    name
  }
}
"#;
        let parsed = parse(src);
        assert!(parsed.requests[0].graphql);
        let BodySpec::Inline(body) = &parsed.requests[0].body else {
            panic!("expected inline body");
        };
        let value: serde_json::Value = serde_json::from_str(body).unwrap();
        assert!(value["query"]
            .as_str()
            .unwrap()
            .contains("query GetUser($id: ID!)"));
        assert!(value.get("variables").is_none());
    }

    #[test]
    fn does_not_treat_graphql_word_as_keyword() {
        let src = r#"
### plain text body
POST http://localhost:8080/echo

graphqlfoo is not a keyword
"#;
        let parsed = parse(src);
        assert!(!parsed.requests[0].graphql);
        assert_eq!(
            parsed.requests[0].body,
            BodySpec::Inline("graphqlfoo is not a keyword".to_string())
        );
    }
}
