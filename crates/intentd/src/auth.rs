//! Bearer-token protection for routes that change state or launch programs.
//!
//! 1.x served `/v1/restore` (which spawns editors, browsers and terminals) to any
//! local process. The token lives in a 0600 file only the user can read; the Tauri
//! app and `intentctl` read it, browser/editor companions only need the open routes.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

#[derive(Clone)]
pub struct Token(Arc<str>);

impl Token {
    /// Loads the existing token or writes a fresh one to the first usable location.
    pub fn load_or_create(paths: &[PathBuf]) -> Result<(Token, PathBuf)> {
        for path in paths {
            if let Ok(existing) = fs::read_to_string(path) {
                let existing = existing.trim();
                if existing.len() >= 32 {
                    return Ok((Token(existing.into()), path.clone()));
                }
            }
        }
        let token = generate()?;
        let mut last_err = None;
        for path in paths {
            match write_private(path, &token) {
                Ok(()) => return Ok((Token(token.into()), path.clone())),
                Err(err) => last_err = Some(err),
            }
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no token path configured")))
    }

    fn matches(&self, presented: &str) -> bool {
        constant_time_eq(self.0.as_bytes(), presented.as_bytes())
    }
}

fn generate() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("getrandom: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn write_private(path: &Path, token: &str) -> Result<()> {
    let dir = path.parent().context("token path has no parent")?;
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        let tmp = dir.join(".token.tmp");
        let mut file = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        file.write_all(token.as_bytes())?;
        file.sync_all()?;
        fs::rename(&tmp, path)?;
    }
    #[cfg(not(unix))]
    fs::write(path, token)?;
    Ok(())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// axum middleware: rejects requests without the daemon's bearer token.
pub async fn require_token(State(token): State<Token>, request: Request, next: Next) -> Response {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match presented {
        Some(p) if token.matches(p.trim()) => next.run(request).await,
        _ => (StatusCode::UNAUTHORIZED, axum::Json(serde_json::json!({ "detail": "missing or invalid token" })))
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_then_reuses_private_token() {
        let dir = std::env::temp_dir().join(format!("intent-auth-{}", std::process::id()));
        let path = dir.join("intent/token");
        let (first, at) = Token::load_or_create(std::slice::from_ref(&path)).unwrap();
        assert_eq!(at, path);
        let (second, _) = Token::load_or_create(&[path.clone()]).unwrap();
        assert!(first.matches(&second.0));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn comparison() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
