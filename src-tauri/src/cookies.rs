use crate::models::CookieInfo;
use reqwest::header::HeaderValue;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// 可持久化、可枚举的 Cookie 存储，包装 cookie_store::CookieStore 并实现
/// reqwest 的 CookieStore trait，可直接作为 reqwest 的 cookie provider。
#[derive(Default)]
pub struct SharedCookieStore {
    inner: Mutex<cookie_store::CookieStore>,
}

impl SharedCookieStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl reqwest::cookie::CookieStore for SharedCookieStore {
    fn set_cookies(&self, cookie_headers: &mut dyn Iterator<Item = &HeaderValue>, url: &url::Url) {
        let iter = cookie_headers
            .filter_map(|value| value.to_str().ok())
            .filter_map(|text| cookie_store::RawCookie::parse(text).ok().map(|c| c.into_owned()));
        if let Ok(mut guard) = self.inner.lock() {
            guard.store_response_cookies(iter, url);
        }
    }

    fn cookies(&self, url: &url::Url) -> Option<HeaderValue> {
        let guard = self.inner.lock().ok()?;
        let header = guard
            .get_request_values(url)
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ");
        if header.is_empty() {
            None
        } else {
            HeaderValue::from_str(&header).ok()
        }
    }
}

fn store_path(root: &Path) -> PathBuf {
    root.join(".http-client").join("cookies.json")
}

/// 打开工作区时恢复 Cookie（含会话 Cookie）。
pub fn load(root: &Path) -> Arc<SharedCookieStore> {
    let store = std::fs::File::open(store_path(root))
        .ok()
        .map(BufReader::new)
        .and_then(|reader| cookie_store::serde::json::load_all(reader).ok())
        .unwrap_or_default();
    Arc::new(SharedCookieStore {
        inner: Mutex::new(store),
    })
}

pub fn save(root: &Path, store: &SharedCookieStore) {
    let path = store_path(root);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(mut file) = std::fs::File::create(path) else {
        return;
    };
    if let Ok(guard) = store.inner.lock() {
        let _ = cookie_store::serde::json::save_incl_expired_and_nonpersistent(&guard, &mut file);
    }
}

pub fn list(store: &SharedCookieStore) -> Vec<CookieInfo> {
    let Ok(guard) = store.inner.lock() else {
        return Vec::new();
    };
    let mut items: Vec<CookieInfo> = guard
        .iter_any()
        .map(|cookie| CookieInfo {
            name: cookie.name().to_string(),
            value: cookie.value().to_string(),
            domain: match &cookie.domain {
                cookie_store::CookieDomain::HostOnly(host) => host.clone(),
                cookie_store::CookieDomain::Suffix(suffix) => suffix.clone(),
                other => format!("{other:?}"),
            },
            path: cookie.path.to_string(),
            expires: match cookie.expires {
                cookie_store::CookieExpiration::SessionEnd => None,
                cookie_store::CookieExpiration::AtUtc(at) => Some(at.to_string()),
            },
        })
        .collect();
    items.sort_by(|a, b| a.domain.cmp(&b.domain).then(a.name.cmp(&b.name)));
    items
}

pub fn clear(root: &Path, store: &SharedCookieStore) {
    if let Ok(mut guard) = store.inner.lock() {
        *guard = cookie_store::CookieStore::default();
    }
    save(root, store);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_and_lists_cookies() {
        let base =
            std::env::temp_dir().join(format!("httpdeck-cookies-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&base).unwrap();

        let store = load(&base);
        let url = url::Url::parse("http://example.com/api").unwrap();
        {
            // 模拟服务端 Set-Cookie
            let mut guard = store.inner.lock().unwrap();
            guard.store_response_cookies(
                vec![cookie_store::RawCookie::parse("sid=abc; Path=/").unwrap()].into_iter(),
                &url,
            );
        }
        let cookies = list(&store);
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].name, "sid");
        assert_eq!(cookies[0].value, "abc");

        // 请求时会带上 Cookie
        let header = reqwest::cookie::CookieStore::cookies(store.as_ref(), &url)
            .expect("应有 Cookie 头");
        assert!(header.to_str().unwrap().contains("sid=abc"));

        save(&base, &store);

        // 重新加载后仍在
        let reloaded = load(&base);
        let cookies = list(&reloaded);
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].name, "sid");

        clear(&base, &reloaded);
        assert!(list(&reloaded).is_empty());

        std::fs::remove_dir_all(&base).ok();
    }
}
