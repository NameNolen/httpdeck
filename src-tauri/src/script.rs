use crate::models::TestResult;
use rquickjs::{Context, Ctx, Exception, Runtime};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Default)]
pub struct ScriptRequest {
    pub method: String,
    pub url: String,
    pub body: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ScriptResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body_text: String,
    pub mime_type: Option<String>,
    pub charset: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ScriptOutcome {
    pub globals: HashMap<String, String>,
    pub request_vars: HashMap<String, String>,
    pub tests: Vec<TestResult>,
    pub logs: Vec<String>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StateSeed<'a> {
    globals: &'a HashMap<String, String>,
    request_vars: &'a HashMap<String, String>,
    tests: Vec<TestResult>,
    logs: Vec<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct RawState {
    #[serde(default)]
    globals: HashMap<String, String>,
    #[serde(default)]
    request_vars: HashMap<String, String>,
    #[serde(default)]
    tests: Vec<TestResult>,
    #[serde(default)]
    logs: Vec<String>,
}

pub fn run_scripts(
    scripts: &[String],
    globals: &HashMap<String, String>,
    request_vars: &HashMap<String, String>,
    request: &ScriptRequest,
    response: Option<&ScriptResponse>,
) -> ScriptOutcome {
    if scripts.is_empty() {
        return ScriptOutcome {
            globals: globals.clone(),
            request_vars: request_vars.clone(),
            ..Default::default()
        };
    }

    let runtime = match Runtime::new() {
        Ok(rt) => rt,
        Err(err) => {
            return ScriptOutcome {
                globals: globals.clone(),
                request_vars: request_vars.clone(),
                error: Some(format!("脚本引擎初始化失败: {err}")),
                ..Default::default()
            }
        }
    };
    let context = match Context::full(&runtime) {
        Ok(ctx) => ctx,
        Err(err) => {
            return ScriptOutcome {
                globals: globals.clone(),
                request_vars: request_vars.clone(),
                error: Some(format!("脚本上下文创建失败: {err}")),
                ..Default::default()
            }
        }
    };

    let prelude = build_prelude(globals, request_vars, request, response);
    let mut error: Option<String> = None;

    let state_json = context.with(|ctx| {
        let _: () = match ctx.eval(prelude.as_bytes()) {
            Ok(value) => value,
            Err(err) => {
                error = Some(format!("脚本初始化失败: {}", describe_error(&ctx, err)));
                return None;
            }
        };
        for (index, script) in scripts.iter().enumerate() {
            if let Err(err) = ctx.eval::<(), _>(script.as_bytes()) {
                error = Some(format!(
                    "脚本第 {} 段执行失败: {}",
                    index + 1,
                    describe_error(&ctx, err)
                ));
                break;
            }
        }
        match ctx.eval::<String, _>("JSON.stringify(__httpdeck_state)".as_bytes()) {
            Ok(value) => Some(value),
            Err(err) => {
                if error.is_none() {
                    error = Some(format!("读取脚本状态失败: {}", describe_error(&ctx, err)));
                }
                None
            }
        }
    });

    let mut outcome = ScriptOutcome {
        globals: globals.clone(),
        request_vars: request_vars.clone(),
        error,
        ..Default::default()
    };

    if let Some(state_json) = state_json {
        if let Ok(raw) = serde_json::from_str::<RawState>(&state_json) {
            outcome.globals = raw.globals;
            outcome.request_vars = raw.request_vars;
            outcome.tests = raw.tests;
            outcome.logs = raw.logs;
        } else {
            outcome.error = Some("脚本返回状态无法解析".to_string());
        }
    }

    outcome
}

fn describe_error(ctx: &Ctx<'_>, err: rquickjs::Error) -> String {
    if matches!(err, rquickjs::Error::Exception) {
        let value = ctx.catch();
        if let Some(obj) = value.as_object() {
            if let Some(exception) = Exception::from_object(obj.clone()) {
                let message = exception.message().unwrap_or_default();
                let stack = exception.stack().unwrap_or_default();
                if !message.is_empty() {
                    return if stack.is_empty() {
                        message
                    } else {
                        format!("{message}\n{stack}")
                    };
                }
            }
        }
        if let Some(text) = value.as_string().and_then(|text| text.to_string().ok()) {
            return text;
        }
        return "脚本抛出异常".to_string();
    }
    err.to_string()
}

fn build_prelude(
    globals: &HashMap<String, String>,
    request_vars: &HashMap<String, String>,
    request: &ScriptRequest,
    response: Option<&ScriptResponse>,
) -> String {
    let seed = StateSeed {
        globals,
        request_vars,
        tests: Vec::new(),
        logs: Vec::new(),
    };
    let state_json = serde_json::to_string(&seed).unwrap_or_else(|_| "{}".to_string());
    let request_body = match &request.body {
        Some(body) => serde_json::to_string(body).unwrap_or_else(|_| "null".to_string()),
        None => "null".to_string(),
    };

    let response_js = match response {
        None => "var response = null;".to_string(),
        Some(resp) => {
            let mut headers: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for (name, value) in &resp.headers {
                headers
                    .entry(name.to_ascii_lowercase())
                    .or_default()
                    .push(value.clone());
            }
            let headers_json = serde_json::to_string(&headers).unwrap_or_else(|_| "{}".to_string());
            let mime = resp
                .mime_type
                .as_ref()
                .map(|m| serde_json::to_string(m).unwrap_or_else(|_| "null".to_string()))
                .unwrap_or_else(|| "null".to_string());
            let charset = resp
                .charset
                .as_ref()
                .map(|c| serde_json::to_string(c).unwrap_or_else(|_| "null".to_string()))
                .unwrap_or_else(|| "null".to_string());
            let body_json = response_body_value(resp);

            format!(
                r#"
var __httpdeck_headers = {headers_json};
var response = {{
  status: {status},
  body: {body},
  bodyText: {body_text},
  headers: {{
    valueOf: function(name) {{
      var list = __httpdeck_headers[String(name).toLowerCase()];
      return list && list.length ? list[0] : null;
    }},
    valuesOf: function(name) {{
      return __httpdeck_headers[String(name).toLowerCase()] || [];
    }}
  }},
  contentType: {{ mimeType: {mime}, charset: {charset} }}
}};
"#,
                headers_json = headers_json,
                status = resp.status,
                body = body_json,
                body_text = serde_json::to_string(&resp.body_text)
                    .unwrap_or_else(|_| "null".to_string()),
                mime = mime,
                charset = charset,
            )
        }
    };

    format!(
        r#"
var __httpdeck_state = {state_json};
function __httpdeck_str(value) {{
  return value === undefined || value === null ? "" : String(value);
}}
var client = {{
  global: {{
    set: function(name, value) {{
      __httpdeck_state.globals[String(name)] = __httpdeck_str(value);
    }},
    get: function(name) {{
      var key = String(name);
      return Object.prototype.hasOwnProperty.call(__httpdeck_state.globals, key)
        ? __httpdeck_state.globals[key]
        : null;
    }},
    unset: function(name) {{ delete __httpdeck_state.globals[String(name)]; }},
    clear: function(name) {{ delete __httpdeck_state.globals[String(name)]; }},
    isEmpty: function() {{ return Object.keys(__httpdeck_state.globals).length === 0; }}
  }},
  test: function(name, fn) {{
    try {{
      fn();
      __httpdeck_state.tests.push({{ name: String(name), passed: true, message: "" }});
    }} catch (err) {{
      __httpdeck_state.tests.push({{
        name: String(name),
        passed: false,
        message: String(err && err.message ? err.message : err)
      }});
    }}
  }},
  assert: function(condition, message) {{
    if (!condition) {{
      throw new Error(message === undefined ? "Assertion failed" : String(message));
    }}
  }},
  log: function() {{
    var parts = [];
    for (var i = 0; i < arguments.length; i++) {{
      var item = arguments[i];
      if (item !== null && typeof item === "object") {{
        try {{ parts.push(JSON.stringify(item)); }}
        catch (e) {{ parts.push(String(item)); }}
      }} else {{
        parts.push(String(item));
      }}
    }}
    __httpdeck_state.logs.push(parts.join(" "));
  }}
}};
var console = {{ log: client.log, info: client.log, warn: client.log, error: client.log }};
var request = {{
  method: {method},
  url: {url},
  body: {body},
  headers: {{}},
  variables: {{
    set: function(name, value) {{
      __httpdeck_state.requestVars[String(name)] = __httpdeck_str(value);
    }},
    get: function(name) {{
      var key = String(name);
      return Object.prototype.hasOwnProperty.call(__httpdeck_state.requestVars, key)
        ? __httpdeck_state.requestVars[key]
        : null;
    }},
    unset: function(name) {{ delete __httpdeck_state.requestVars[String(name)]; }},
    clear: function(name) {{ delete __httpdeck_state.requestVars[String(name)]; }}
  }}
}};
{response_js}
"#,
        state_json = state_json,
        method = serde_json::to_string(&request.method).unwrap_or_else(|_| "\"\"".to_string()),
        url = serde_json::to_string(&request.url).unwrap_or_else(|_| "\"\"".to_string()),
        body = request_body,
        response_js = response_js,
    )
}

fn response_body_value(resp: &ScriptResponse) -> String {
    let is_json = resp
        .mime_type
        .as_ref()
        .map(|m| m.contains("json"))
        .unwrap_or(false);
    if is_json {
        if let Ok(parsed) = serde_json::from_str::<Value>(&resp.body_text) {
            if let Ok(text) = serde_json::to_string(&parsed) {
                return text;
            }
        }
    }
    serde_json::to_string(&resp.body_text).unwrap_or_else(|_| "null".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supports_global_and_tests() {
        let script = r#"
client.global.set("token", "abc");
client.test("status", function() {
  client.assert(response.status === 200, "bad status");
});
client.test("body", function() {
  client.assert(response.body.id === 42, "bad id");
});
client.log("hello", 1);
"#;
        let response = ScriptResponse {
            status: 200,
            body_text: r#"{"id":42}"#.to_string(),
            mime_type: Some("application/json".to_string()),
            charset: Some("UTF-8".to_string()),
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
        };
        let outcome = run_scripts(
            &[script.to_string()],
            &HashMap::new(),
            &HashMap::new(),
            &ScriptRequest {
                method: "GET".to_string(),
                url: "http://localhost".to_string(),
                body: None,
            },
            Some(&response),
        );
        assert!(outcome.error.is_none(), "{:?}", outcome.error);
        assert_eq!(outcome.globals.get("token").unwrap(), "abc");
        assert_eq!(outcome.tests.len(), 2);
        assert!(outcome.tests.iter().all(|t| t.passed));
        assert_eq!(outcome.logs.first().unwrap(), "hello 1");
    }

    #[test]
    fn reports_failed_assertions() {
        let script = r#"
client.test("status", function() {
  client.assert(response.status === 201, "expected 201");
});
"#;
        let response = ScriptResponse {
            status: 200,
            ..Default::default()
        };
        let outcome = run_scripts(
            &[script.to_string()],
            &HashMap::new(),
            &HashMap::new(),
            &ScriptRequest::default(),
            Some(&response),
        );
        assert_eq!(outcome.tests.len(), 1);
        assert!(!outcome.tests[0].passed);
        assert!(outcome.tests[0].message.contains("expected 201"));
    }

    #[test]
    fn sets_request_variables() {
        let script = r#"request.variables.set("id", "7");"#;
        let mut request_vars = HashMap::new();
        request_vars.insert("old".to_string(), "1".to_string());
        let outcome = run_scripts(
            &[script.to_string()],
            &HashMap::new(),
            &request_vars,
            &ScriptRequest::default(),
            None,
        );
        assert_eq!(outcome.request_vars.get("id").unwrap(), "7");
        assert_eq!(outcome.request_vars.get("old").unwrap(), "1");
    }

    #[test]
    fn captures_script_errors() {
        let outcome = run_scripts(
            &["throw new Error('boom');".to_string()],
            &HashMap::new(),
            &HashMap::new(),
            &ScriptRequest::default(),
            None,
        );
        assert!(outcome.error.unwrap().contains("boom"));
    }
}
