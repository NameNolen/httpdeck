//! 端到端集成测试：解析 .http 文本 → 变量替换 → 本地 HTTP 服务器执行 →
//! 后置脚本（client.test / client.global.set）→ Cookie 跨请求保留。
//!
//! 本地服务器直接在 TCP 层收发原始 HTTP，不引入额外依赖。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use httpdeck_lib::cookies::SharedCookieStore;
use httpdeck_lib::executor::execute_request;
use httpdeck_lib::parser;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Default)]
struct ServerState {
    /// /me 请求收到的 Cookie / Authorization 头
    me_headers: Mutex<Vec<(String, String)>>,
}

fn http_response(extra_headers: &[&str], body: &str) -> Vec<u8> {
    let mut out = String::from("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n");
    for header in extra_headers {
        out.push_str(header);
        out.push_str("\r\n");
    }
    out.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
    let mut bytes = out.into_bytes();
    bytes.extend_from_slice(body.as_bytes());
    bytes
}

/// 返回完整请求（头 + 体）的字节长度
fn find_request_end(data: &[u8]) -> Option<usize> {
    let pos = data.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&data[..pos]);
    let content_length = head
        .lines()
        .skip(1)
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.trim().eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);
    Some(pos + 4 + content_length)
}

async fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<String>> {
    let mut data: Vec<u8> = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        if let Some(end) = find_request_end(&data) {
            return Ok(Some(String::from_utf8_lossy(&data[..end]).into_owned()));
        }
        let n = stream.read(&mut buf).await?;
        if n == 0 {
            return Ok(if data.is_empty() {
                None
            } else {
                Some(String::from_utf8_lossy(&data).into_owned())
            });
        }
        data.extend_from_slice(&buf[..n]);
    }
}

async fn handle_conn(mut stream: TcpStream, state: Arc<ServerState>) -> std::io::Result<()> {
    loop {
        let Some(request) = read_request(&mut stream).await? else {
            break;
        };
        let head_end = request.find("\r\n\r\n").unwrap_or(request.len());
        let head = &request[..head_end];
        let request_line = head.lines().next().unwrap_or("");
        let path = request_line.split_whitespace().nth(1).unwrap_or("");
        let headers: Vec<(String, String)> = head
            .lines()
            .skip(1)
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                Some((name.trim().to_string(), value.trim().to_string()))
            })
            .collect();

        let response = if path.starts_with("/login") {
            http_response(
                &["Set-Cookie: session=abc123; Path=/"],
                "{\"token\": \"tok_123\"}",
            )
        } else if path.starts_with("/me") {
            for (name, value) in &headers {
                let lower = name.to_ascii_lowercase();
                if lower == "cookie" || lower == "authorization" {
                    state
                        .me_headers
                        .lock()
                        .unwrap()
                        .push((name.clone(), value.clone()));
                }
            }
            http_response(&[], "{\"ok\": true}")
        } else {
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec()
        };
        stream.write_all(&response).await?;
    }
    Ok(())
}

async fn spawn_server(state: Arc<ServerState>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let state = state.clone();
                    tokio::spawn(async move {
                        let _ = handle_conn(stream, state).await;
                    });
                }
                Err(_) => break,
            }
        }
    });
    port
}

fn temp_workspace() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "httpdeck-e2e-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn end_to_end_parse_variables_scripts_and_cookies() {
    let state = Arc::new(ServerState::default());
    let port = spawn_server(state.clone()).await;

    let content = r#"
@host = http://127.0.0.1:__PORT__

### 登录
POST {{host}}/login
Content-Type: application/json

{"user": "alice"}

> {%
    client.test("登录成功", function () {
        client.assert(response.status === 200, "状态码 " + response.status);
        client.assert(response.body.token, "缺少 token");
    });
    client.global.set("token", response.body.token);
%}

### 获取个人信息
GET {{host}}/me
Authorization: Bearer {{token}}

> {%
    client.test("会话可用", function () {
        client.assert(response.status === 200, "状态码 " + response.status);
    });
%}
"#
    .replace("__PORT__", &port.to_string());

    let parsed = parser::parse(&content);
    assert_eq!(parsed.requests.len(), 2);
    let file_vars: HashMap<String, String> = parsed.variables.iter().cloned().collect();
    let base_dir = temp_workspace();

    let cookies = Arc::new(SharedCookieStore::new());
    let mut globals = HashMap::new();

    // 请求 1：登录。校验状态、脚本测试结果与全局变量写入
    let first = execute_request(
        &parsed.requests[0],
        &file_vars,
        &HashMap::new(),
        &mut globals,
        cookies.clone(),
        &base_dir,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(first.status, 200, "登录失败: {:?}", first.error);
    assert_eq!(first.url, format!("http://127.0.0.1:{port}/login"));
    assert!(first.tests.iter().all(|test| test.passed), "{:?}", first.tests);
    assert_eq!(globals.get("token").map(String::as_str), Some("tok_123"));

    // 请求 2：使用 {{token}} 变量与登录留下的会话 Cookie
    let second = execute_request(
        &parsed.requests[1],
        &file_vars,
        &HashMap::new(),
        &mut globals,
        cookies.clone(),
        &base_dir,
        None,
        None,
        None,
    )
    .await;
    assert_eq!(second.status, 200, "获取 /me 失败: {:?}", second.error);
    assert_eq!(
        second.url,
        format!("http://127.0.0.1:{port}/me"),
        "变量替换失败"
    );
    assert!(
        second.tests.iter().all(|test| test.passed),
        "{:?}",
        second.tests
    );

    // 服务器确实收到了登录时下发的 Cookie 与脚本写入的 Bearer token
    let received = state.me_headers.lock().unwrap().clone();
    let cookie = received
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("cookie"))
        .map(|(_, value)| value.clone());
    let auth = received
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        .map(|(_, value)| value.clone());
    assert_eq!(
        cookie.as_deref().map(str::to_string),
        Some("session=abc123".to_string()),
        "Cookie 未跨请求保留，实际收到: {:?}",
        received
    );
    assert_eq!(auth.as_deref(), Some("Bearer tok_123"));

    let _ = std::fs::remove_dir_all(&base_dir);
}
