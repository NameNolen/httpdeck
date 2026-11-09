use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;
use uuid::Uuid;

pub struct VariableContext<'a> {
    pub file_vars: &'a HashMap<String, String>,
    pub env_vars: &'a HashMap<String, String>,
    pub globals: &'a HashMap<String, String>,
    pub request_vars: &'a HashMap<String, String>,
}

impl<'a> VariableContext<'a> {
    fn lookup(&self, key: &str) -> Option<String> {
        if let Some(v) = self.request_vars.get(key) {
            return Some(v.clone());
        }
        if let Some(v) = self.globals.get(key) {
            return Some(v.clone());
        }
        if let Some(v) = self.env_vars.get(key) {
            return Some(v.clone());
        }
        if let Some(v) = self.file_vars.get(key) {
            return Some(v.clone());
        }
        dynamic_value(key)
    }
}

fn var_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{\{\s*([^{}]+?)\s*\}\}").expect("valid regex"))
}

pub fn resolve(input: &str, ctx: &VariableContext) -> String {
    if !input.contains("{{") {
        return input.to_string();
    }
    let mut current = input.to_string();
    for _ in 0..20 {
        let next = var_regex()
            .replace_all(&current, |caps: &regex::Captures| match ctx.lookup(&caps[1]) {
                Some(value) => value,
                None => caps[0].to_string(),
            })
            .to_string();
        if next == current {
            break;
        }
        current = next;
    }
    current
}

fn dynamic_value(name: &str) -> Option<String> {
    match name {
        "$uuid" | "$random.uuid" => Some(Uuid::new_v4().to_string()),
        "$timestamp" => Some(chrono::Utc::now().timestamp().to_string()),
        "$isoTimestamp" => Some(chrono::Utc::now().to_rfc3339()),
        "$randomInt" => Some((Uuid::new_v4().as_u128() % 1000).to_string()),
        "$random.email" => Some(format!("{}@example.com", short_token(8))),
        _ => {
            if let Some(arg) = name.strip_prefix("$random.alphabetic(") {
                let n = arg.trim_end_matches(')').trim().parse::<usize>().ok()?;
                return Some(random_from("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ", n));
            }
            if let Some(arg) = name.strip_prefix("$random.alphanumeric(") {
                let n = arg.trim_end_matches(')').trim().parse::<usize>().ok()?;
                return Some(random_from(
                    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
                    n,
                ));
            }
            if let Some(arg) = name.strip_prefix("$random.hexadecimal(") {
                let n = arg.trim_end_matches(')').trim().parse::<usize>().ok()?;
                return Some(random_from("0123456789abcdef", n));
            }
            if let Some(arg) = name.strip_prefix("$processEnv(") {
                let key = arg.trim_end_matches(')').trim();
                return std::env::var(key).ok();
            }
            None
        }
    }
}

fn short_token(n: usize) -> String {
    let id = Uuid::new_v4().simple().to_string();
    id.chars().take(n).collect()
}

fn random_from(alphabet: &str, n: usize) -> String {
    let chars: Vec<char> = alphabet.chars().collect();
    let mut seed = Uuid::new_v4().as_u128();
    let mut out = String::with_capacity(n);
    for _ in 0..n {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let idx = ((seed >> 64) as usize) % chars.len();
        out.push(chars[idx]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_nested_variables() {
        let file = HashMap::from([
            ("host".to_string(), "http://localhost:8080".to_string()),
            ("base".to_string(), "{{host}}/api".to_string()),
        ]);
        let env = HashMap::from([("token".to_string(), "abc".to_string())]);
        let globals = HashMap::new();
        let request = HashMap::new();
        let ctx = VariableContext {
            file_vars: &file,
            env_vars: &env,
            globals: &globals,
            request_vars: &request,
        };
        assert_eq!(resolve("{{base}}/users?t={{token}}", &ctx), "http://localhost:8080/api/users?t=abc");
        assert_eq!(resolve("{{unknown}}", &ctx), "{{unknown}}");
    }

    #[test]
    fn request_variables_win() {
        let file = HashMap::from([("value".to_string(), "file".to_string())]);
        let env = HashMap::from([("value".to_string(), "env".to_string())]);
        let globals = HashMap::from([("value".to_string(), "global".to_string())]);
        let request = HashMap::from([("value".to_string(), "request".to_string())]);
        let ctx = VariableContext {
            file_vars: &file,
            env_vars: &env,
            globals: &globals,
            request_vars: &request,
        };
        assert_eq!(resolve("{{value}}", &ctx), "request");
    }

    #[test]
    fn expands_dynamic_variables() {
        let empty = HashMap::new();
        let ctx = VariableContext {
            file_vars: &empty,
            env_vars: &empty,
            globals: &empty,
            request_vars: &empty,
        };
        let uuid = resolve("{{$uuid}}", &ctx);
        assert_eq!(uuid.len(), 36);
        let ts = resolve("{{$timestamp}}", &ctx);
        assert!(ts.parse::<i64>().is_ok());
        let alpha = resolve("{{$random.alphabetic(6)}}", &ctx);
        assert_eq!(alpha.len(), 6);
    }
}
