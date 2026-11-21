//! Browser-facing guards: the Host check against DNS rebinding, the Origin check
//! against cross-site writes, and the JSON content-type check for ingest.
//!
//! The token protects every read of captured data, but the open companion routes and
//! the ingest route have no token. A web page can still reach `127.0.0.1` from the
//! user's browser, so these checks close the gaps the token leaves.

use axum::extract::Request;
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::error::ApiError;

/// Fixed origins that may change state from a browser context: the Tauri app.
const APP_ORIGINS: &[&str] = &["tauri://localhost", "http://tauri.localhost"];
/// The Tauri dev server. Debug builds only, as in the CORS list.
const DEV_ORIGINS: &[&str] = &["http://localhost:1420", "http://127.0.0.1:1420"];

/// Whether `host` (a `Host` header value) names this machine's loopback interface.
///
/// A DNS-rebinding page reaches the daemon under its own hostname, so it sends a
/// foreign `Host`. A port is optional and any port is fine.
pub fn is_loopback_host(host: &str) -> bool {
    // `rest` is empty or `:<port>`.
    let (name, rest, bracketed) = match host.strip_prefix('[') {
        Some(inner) => match inner.split_once(']') {
            Some((addr, rest)) => (addr, rest, true),
            None => return false,
        },
        None => match host.find(':') {
            Some(i) => (&host[..i], &host[i..], false),
            None => (host, "", false),
        },
    };
    let port_ok = rest.is_empty()
        || rest.strip_prefix(':').is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()));
    let name_ok = if bracketed { name == "::1" } else { name.eq_ignore_ascii_case("localhost") || name == "127.0.0.1" };
    port_ok && name_ok
}

/// Whether a browser `Origin` may call a state-changing route.
///
/// Any `moz-extension://` origin is allowed: the Firefox companion's UUID differs per
/// install, and a web page cannot send that scheme. Web origins, `null` (sandboxed
/// frames, `file:`) and everything else are refused.
pub fn is_allowed_origin(origin: &str) -> bool {
    origin.strip_prefix("moz-extension://").is_some_and(|id| !id.is_empty() && !id.contains('/'))
        || APP_ORIGINS.contains(&origin)
        || (cfg!(debug_assertions) && DEV_ORIGINS.contains(&origin))
}

/// `application/json`, with optional parameters such as `charset`.
pub fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
}

fn changes_state(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// Router-wide: refuse a request whose `Host` is not loopback (421).
pub async fn host_layer(request: Request, next: Next) -> Response {
    let host = request.headers().get(header::HOST).and_then(|v| v.to_str().ok());
    if host.is_some_and(is_loopback_host) {
        next.run(request).await
    } else {
        ApiError::new(StatusCode::MISDIRECTED_REQUEST, "host must be 127.0.0.1, localhost or [::1]").into_response()
    }
}

/// Router-wide: refuse a state-changing request from a browser origin that is not a
/// companion or the app (403). Clients without an `Origin` (the shell hook, intentctl,
/// the VS Code companion, the app's Rust side) are not browsers and pass.
pub async fn origin_layer(request: Request, next: Next) -> Response {
    if changes_state(request.method()) {
        let origin = request.headers().get(header::ORIGIN).map(|v| v.to_str().unwrap_or(""));
        if origin.is_some_and(|o| !is_allowed_origin(o)) {
            return ApiError::new(StatusCode::FORBIDDEN, "origin not allowed").into_response();
        }
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn loopback_hosts() {
        for host in ["127.0.0.1", "127.0.0.1:9477", "localhost", "LocalHost:1", "[::1]", "[::1]:9477"] {
            assert!(is_loopback_host(host), "{host}");
        }
        for host in [
            "",
            "evil.example",
            "evil.example:9477",
            "127.0.0.1.evil.example",
            "localhost.evil.example",
            "127.0.0.2",
            "0.0.0.0:9477",
            "[::2]",
            "[::1",
            "[::1]x",
            "::1",
            "127.0.0.1:",
            "127.0.0.1:x",
            "localhost:9477:1",
            "[localhost]",
        ] {
            assert!(!is_loopback_host(host), "{host}");
        }
    }

    #[test]
    fn allowed_origins() {
        for origin in ["moz-extension://3f2a-uuid", "tauri://localhost", "http://tauri.localhost"] {
            assert!(is_allowed_origin(origin), "{origin}");
        }
        for origin in [
            "https://evil.example",
            "null",
            "",
            "moz-extension://",
            "moz-extension://a/b",
            "chrome-extension://abc",
            "http://localhost:5173",
            "http://127.0.0.1:9477",
            "tauri://localhost.evil.example",
            "file://",
        ] {
            assert!(!is_allowed_origin(origin), "{origin}");
        }
        assert_eq!(is_allowed_origin("http://localhost:1420"), cfg!(debug_assertions));
    }

    #[test]
    fn json_content_types() {
        let with = |v: &'static str| {
            let mut h = HeaderMap::new();
            h.insert(header::CONTENT_TYPE, HeaderValue::from_static(v));
            is_json(&h)
        };
        assert!(with("application/json"));
        assert!(with("application/json; charset=utf-8"));
        assert!(with("Application/JSON"));
        assert!(!with("text/plain"));
        assert!(!with("text/plain;charset=UTF-8"));
        assert!(!with("application/jsonx"));
        assert!(!with("multipart/form-data; boundary=x"));
        assert!(!is_json(&HeaderMap::new()));
    }
}
