use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

#[derive(Debug, Default, Clone)]
pub struct EnvSet {
    pub names: Vec<String>,
    pub vars: BTreeMap<String, HashMap<String, String>>,
}

impl EnvSet {
    pub fn vars_for(&self, name: &str) -> HashMap<String, String> {
        self.vars.get(name).cloned().unwrap_or_default()
    }
}

type EnvMap = BTreeMap<String, Map<String, Value>>;

pub fn load_envs(root: &Path) -> EnvSet {
    let public = read_env_file(&root.join("http-client.env.json"));
    let private = read_env_file(&root.join("http-client.private.env.json"));
    merge(public, private)
}

/// 目录级环境：从工作区根到文件所在目录逐层读取 env 文件，
/// 越靠近文件的目录优先级越高（覆盖同名变量）。
/// 单层内的优先级与 load_envs 一致：$shared(公有) < 具名环境(公有) < $shared(私有) < 具名环境(私有)。
pub fn load_envs_chain(root: &Path, dir: &Path) -> EnvSet {
    let mut chain = Vec::new();
    let mut current = Some(dir);
    while let Some(d) = current {
        chain.push(d.to_path_buf());
        if d == root {
            break;
        }
        current = d.parent();
    }
    chain.reverse();

    let mut shared: HashMap<String, String> = HashMap::new();
    let mut layers: BTreeMap<String, Vec<HashMap<String, String>>> = BTreeMap::new();
    for level in chain {
        let public = read_env_file(&level.join("http-client.env.json"));
        let private = read_env_file(&level.join("http-client.private.env.json"));
        apply_map(&mut shared, public.get("$shared"));
        for (name, map) in &public {
            if name != "$shared" {
                layers.entry(name.clone()).or_default().push(flatten(map));
            }
        }
        apply_map(&mut shared, private.get("$shared"));
        for (name, map) in &private {
            if name != "$shared" {
                layers.entry(name.clone()).or_default().push(flatten(map));
            }
        }
    }

    let mut vars = BTreeMap::new();
    for (name, list) in &layers {
        let mut map = shared.clone();
        for layer in list {
            for (key, value) in layer {
                map.insert(key.clone(), value.clone());
            }
        }
        vars.insert(name.clone(), map);
    }
    let names = layers.into_keys().collect();
    EnvSet { names, vars }
}

fn flatten(source: &Map<String, Value>) -> HashMap<String, String> {
    let mut map = HashMap::new();
    apply_map(&mut map, Some(source));
    map
}

/// 解析 .env 文件：KEY=VALUE 行，支持 # 注释、export 前缀与引号包裹的值。
pub fn parse_dotenv(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let value = value.trim();
        let value = if value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
        {
            &value[1..value.len() - 1]
        } else {
            value
        };
        map.insert(key.to_string(), value.to_string());
    }
    map
}

/// 沿目录链（根 → 文件所在目录）读取所有 .env，近处覆盖远处。
pub fn load_dotenv_chain(root: &Path, dir: &Path) -> HashMap<String, String> {
    let mut chain = Vec::new();
    let mut current = Some(dir);
    while let Some(d) = current {
        chain.push(d.to_path_buf());
        if d == root {
            break;
        }
        current = d.parent();
    }
    chain.reverse();

    let mut map = HashMap::new();
    for level in chain {
        if let Ok(text) = std::fs::read_to_string(level.join(".env")) {
            for (key, value) in parse_dotenv(&text) {
                map.insert(key, value);
            }
        }
    }
    map
}

fn read_env_file(path: &Path) -> EnvMap {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(_) => return EnvMap::new(),
    };
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(_) => return EnvMap::new(),
    };
    let mut result = EnvMap::new();
    if let Value::Object(envs) = value {
        for (name, env_value) in envs {
            if let Value::Object(map) = env_value {
                result.insert(name, map);
            }
        }
    }
    result
}

fn merge(public: EnvMap, private: EnvMap) -> EnvSet {
    let mut names: Vec<String> = public
        .keys()
        .chain(private.keys())
        .filter(|k| k.as_str() != "$shared")
        .cloned()
        .collect();
    names.sort();
    names.dedup();

    let shared_public = public.get("$shared");
    let shared_private = private.get("$shared");

    let mut vars = BTreeMap::new();
    for name in &names {
        let mut map: HashMap<String, String> = HashMap::new();
        apply_map(&mut map, shared_public);
        apply_map(&mut map, public.get(name));
        apply_map(&mut map, shared_private);
        apply_map(&mut map, private.get(name));
        vars.insert(name.clone(), map);
    }

    EnvSet { names, vars }
}

fn apply_map(target: &mut HashMap<String, String>, source: Option<&Map<String, Value>>) {
    let Some(source) = source else { return };
    for (key, value) in source {
        if key == "$shared" {
            continue;
        }
        let rendered = match value {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => n.to_string(),
            other => other.to_string(),
        };
        target.insert(key.clone(), rendered);
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn to_env_map(value: Value) -> EnvMap {
        let mut result = EnvMap::new();
        if let Value::Object(envs) = value {
            for (name, env_value) in envs {
                if let Value::Object(map) = env_value {
                    result.insert(name, map);
                }
            }
        }
        result
    }

    #[test]
    fn merges_shared_and_private() {
        let public = to_env_map(json!({
            "dev": { "host": "http://dev", "token": "public" },
            "prod": { "host": "http://prod" }
        }));
        let private = to_env_map(json!({
            "$shared": { "shared": "yes" },
            "dev": { "token": "private" }
        }));
        let envs = merge(public, private);
        assert_eq!(envs.names, vec!["dev", "prod"]);
        let dev = envs.vars_for("dev");
        assert_eq!(dev.get("host").unwrap(), "http://dev");
        assert_eq!(dev.get("token").unwrap(), "private");
        assert_eq!(dev.get("shared").unwrap(), "yes");
        assert_eq!(envs.vars_for("prod").get("token"), None);
    }

    #[test]
    fn loads_envs_along_directory_chain() {
        let base = std::env::temp_dir().join(format!("httpdeck-env-{}", uuid::Uuid::new_v4()));
        let sub = base.join("api").join("v1");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(
            base.join("http-client.env.json"),
            r#"{ "dev": { "host": "http://root", "token": "root-token" } }"#,
        )
        .unwrap();
        std::fs::write(
            base.join("api").join("http-client.env.json"),
            r#"{ "dev": { "host": "http://sub" } }"#,
        )
        .unwrap();
        std::fs::write(
            sub.join("http-client.private.env.json"),
            r#"{ "$shared": { "secret": "s3" } }"#,
        )
        .unwrap();

        let envs = load_envs_chain(&base, &sub);
        let dev = envs.vars_for("dev");
        // 子目录覆盖根目录的同名变量
        assert_eq!(dev.get("host").unwrap(), "http://sub");
        // 根目录变量保留
        assert_eq!(dev.get("token").unwrap(), "root-token");
        // 最深处的 private $shared 也生效
        assert_eq!(dev.get("secret").unwrap(), "s3");

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn parses_dotenv_and_merges_chain() {
        let parsed = parse_dotenv(
            "# comment\nHOST=http://a\nexport TOKEN=\"t 1\"\nEMPTY=\nQUOTED='q'\nbad line\n",
        );
        assert_eq!(parsed.get("HOST").unwrap(), "http://a");
        assert_eq!(parsed.get("TOKEN").unwrap(), "t 1");
        assert_eq!(parsed.get("EMPTY").unwrap(), "");
        assert_eq!(parsed.get("QUOTED").unwrap(), "q");
        assert!(!parsed.contains_key("bad line"));

        let base = std::env::temp_dir().join(format!("httpdeck-dotenv-{}", uuid::Uuid::new_v4()));
        let sub = base.join("svc");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(base.join(".env"), "HOST=http://root\nA=1\n").unwrap();
        std::fs::write(sub.join(".env"), "HOST=http://sub\nB=2\n").unwrap();

        let vars = load_dotenv_chain(&base, &sub);
        assert_eq!(vars.get("HOST").unwrap(), "http://sub");
        assert_eq!(vars.get("A").unwrap(), "1");
        assert_eq!(vars.get("B").unwrap(), "2");

        std::fs::remove_dir_all(&base).ok();
    }
}
