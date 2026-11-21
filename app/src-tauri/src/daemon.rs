//! HTTP client for `intentd`. Holds the bearer token on the Rust side only;
//! the webview never sees it.

use std::path::PathBuf;
use std::time::Duration;

use reqwest::{Client, Method, StatusCode};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::Mutex;

pub const UNREACHABLE: &str = "Intent isn’t running on this computer.";

#[derive(Debug, Serialize)]
pub struct ApiResponse {
    pub status: u16,
    pub body: Value,
}

pub struct Daemon {
    http: Client,
    base: String,
    token: Mutex<Option<String>>,
}

impl Daemon {
    pub fn new() -> Self {
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(120))
            .no_proxy()
            .build()
            .expect("reqwest client");
        Self { http, base: base_url(), token: Mutex::new(None) }
    }

    /// Proxy one request. `path` must be an absolute path on the daemon.
    pub async fn request(&self, method: &str, path: &str, body: Option<Value>) -> Result<ApiResponse, String> {
        let method = parse_method(method)?;
        validate_path(path)?;
        let first = self.send(method.clone(), path, body.as_ref(), false).await?;
        if first.status == StatusCode::UNAUTHORIZED.as_u16() {
            // The daemon regenerates its token on restart: reload once and retry.
            return self.send(method, path, body.as_ref(), true).await;
        }
        Ok(first)
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        reload: bool,
    ) -> Result<ApiResponse, String> {
        let token = self.token(reload).await;
        let mut req = self.http.request(method, format!("{}{}", self.base, path));
        if let Some(token) = token {
            req = req.bearer_auth(token);
        }
        if let Some(body) = body {
            req = req.json(body);
        }
        let response = req.send().await.map_err(|e| {
            if e.is_connect() || e.is_timeout() { UNREACHABLE.to_string() } else { format!("Request failed: {e}") }
        })?;
        let status = response.status().as_u16();
        let text = response.text().await.map_err(|e| format!("Couldn’t read the response: {e}"))?;
        let body = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::String(text))
        };
        Ok(ApiResponse { status, body })
    }

    async fn token(&self, reload: bool) -> Option<String> {
        let mut guard = self.token.lock().await;
        if reload || guard.is_none() {
            *guard = read_token();
        }
        guard.clone()
    }

    /// Convenience for Rust-side callers (tray): GET and decode.
    pub async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Option<T> {
        let r = self.request("GET", path, None).await.ok()?;
        if !(200..300).contains(&r.status) {
            return None;
        }
        serde_json::from_value(r.body).ok()
    }

    pub async fn post(&self, path: &str, body: Value) -> Result<(), String> {
        let r = self.request("POST", path, Some(body)).await?;
        if (200..300).contains(&r.status) { Ok(()) } else { Err(format!("Request failed ({})", r.status)) }
    }
}

fn base_url() -> String {
    // Allow pointing at another loopback port for development only.
    if let Ok(url) = std::env::var("INTENT_DAEMON_URL") {
        let trimmed = url.trim_end_matches('/');
        if trimmed.starts_with("http://127.0.0.1:") || trimmed.starts_with("http://localhost:") {
            return trimmed.to_string();
        }
    }
    format!("http://127.0.0.1:{}", intent_types::DEFAULT_PORT)
}

fn parse_method(method: &str) -> Result<Method, String> {
    match method.to_ascii_uppercase().as_str() {
        "GET" => Ok(Method::GET),
        "POST" => Ok(Method::POST),
        "PUT" => Ok(Method::PUT),
        "DELETE" => Ok(Method::DELETE),
        other => Err(format!("Unsupported method {other}")),
    }
}

/// Upper bound for the query string. A long search in a non-Latin script
/// grows up to nine times once percent-encoded.
const MAX_QUERY_LEN: usize = 16 * 1024;

/// Only plain absolute paths on the daemon: no scheme, host, traversal or whitespace.
/// Dots are checked in the path only: a search query may contain `..`.
fn validate_path(path: &str) -> Result<(), String> {
    let (route, query) = path.split_once('?').unwrap_or((path, ""));
    let lower = route.to_ascii_lowercase();
    let ok = route.starts_with('/')
        && !route.starts_with("//")
        && !route.contains(':')
        && !route.contains("..")
        && !lower.contains("%2e")
        && !path.contains('#')
        && !path.contains('\\')
        && !path.chars().any(|c| c.is_whitespace() || c.is_control())
        && route.len() < 2048
        && query.len() < MAX_QUERY_LEN;
    if ok { Ok(()) } else { Err("Invalid request path".into()) }
}

/// `$XDG_RUNTIME_DIR/intent/token`, falling back to `~/.local/share/intent/token`.
pub fn token_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        paths.push(PathBuf::from(dir).join("intent").join("token"));
    }
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")));
    if let Some(data) = data {
        paths.push(data.join("intent").join("token"));
    }
    paths
}

fn read_token() -> Option<String> {
    token_paths()
        .into_iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_local_paths() {
        assert!(validate_path("/intents/current").is_ok());
        assert!(validate_path("/intents/search?q=a%20b&limit=12").is_ok());
        assert!(validate_path("http://evil/").is_err());
        assert!(validate_path("//evil.example/x").is_err());
        assert!(validate_path("/v1/../etc").is_err());
        assert!(validate_path("/a b").is_err());
        assert!(validate_path("intents").is_err());
    }

    #[test]
    fn allows_dots_in_the_query() {
        assert!(validate_path("/intents/search?q=axum..routing&limit=12").is_ok());
        assert!(validate_path("/intents/search?q=..%2F..%2Fetc").is_ok());
    }

    #[test]
    fn rejects_traversal_in_the_path() {
        assert!(validate_path("/intents/..").is_err());
        assert!(validate_path("/intents/../v1/status?q=a").is_err());
        assert!(validate_path("/intents/%2e%2e/v1/status").is_err());
        assert!(validate_path("/intents/.%2E?q=a").is_err());
        assert!(validate_path("/intents/search?q=a#..").is_err());
    }

    #[test]
    fn allows_a_long_unicode_query() {
        // 300 CJK characters: 3 bytes each, 9 characters each once percent-encoded.
        let q = "%E6%97%A5".repeat(300);
        assert!(validate_path(&format!("/intents/search?q={q}&limit=30")).is_ok());
    }

    #[test]
    fn rejects_an_oversized_path() {
        assert!(validate_path(&format!("/{}", "a".repeat(2048))).is_err());
        assert!(validate_path(&format!("/intents/search?q={}", "a".repeat(MAX_QUERY_LEN))).is_err());
    }
}
