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
}
