//! The single sensitive-path rule set.
//!
//! 0.1 kept three diverging copies (server `detailed_capture.py`, the VS Code
//! extension's `events.js`, and `collectors/content.py`). This is their union and the
//! only implementation used by the Rust side: editor consent, filesystem content
//! extraction and anything else that must never read or store credential-like files.

use std::path::Path;

/// Substrings that make any path sensitive (matched case-insensitively on the full path).
pub const SENSITIVE_TERMS: &[&str] = &["secret", "credential", "password", "token"];

/// Exact basenames that are always sensitive.
pub const SENSITIVE_NAMES: &[&str] =
    &[".env", "shadow", "gshadow", "passwd", "known_hosts", "authorized_keys", ".netrc", ".pgpass", ".htpasswd"];

/// Basename prefixes of private-key style files.
pub const SENSITIVE_NAME_PREFIXES: &[&str] = &[".env.", "id_rsa", "id_dsa", "id_ecdsa", "id_ed25519"];

/// Basename suffixes of key / certificate-bundle files.
pub const SENSITIVE_SUFFIXES: &[&str] = &[".pem", ".key", ".p12", ".pfx"];

/// Directory components whose whole subtree is sensitive.
pub const SENSITIVE_DIRS: &[&str] = &[".ssh", ".gnupg", ".pki", ".mozilla", ".password-store"];

/// Multi-component directory paths whose whole subtree is sensitive.
/// (0.1 compared these against single components, so they never matched.)
pub const SENSITIVE_DIR_PATHS: &[&str] = &[".config/google-chrome", ".config/chromium"];

/// Virtual filesystems never read by content extraction.
pub const VIRTUAL_ROOTS: &[&str] = &["/proc", "/sys", "/dev", "/run"];

/// `true` when `path` looks like a credential, key or secret store.
///
/// Purely lexical (no filesystem access); callers that care about symlinks should
/// resolve first (see [`crate::paths::resolve_lenient`]).
pub fn is_sensitive_path(path: impl AsRef<Path>) -> bool {
    let normalised = path.as_ref().to_string_lossy().replace('\\', "/").to_lowercase();
    let components: Vec<&str> = normalised.split('/').filter(|part| !part.is_empty()).collect();
    let basename = components.last().copied().unwrap_or("");
    if SENSITIVE_NAMES.contains(&basename)
        || SENSITIVE_NAME_PREFIXES.iter().any(|prefix| basename.starts_with(prefix))
        || SENSITIVE_SUFFIXES.iter().any(|suffix| basename.ends_with(suffix))
    {
        return true;
    }
    if components.iter().any(|part| SENSITIVE_DIRS.contains(part)) {
        return true;
    }
    if SENSITIVE_DIR_PATHS.iter().any(|dir| {
        let wanted: Vec<&str> = dir.split('/').collect();
        components.windows(wanted.len()).any(|window| window == wanted.as_slice())
    }) {
        return true;
    }
    SENSITIVE_TERMS.iter().any(|term| normalised.contains(term))
}

/// `true` for paths under `/proc`, `/sys`, `/dev` or `/run`.
pub fn is_virtual_fs_path(path: impl AsRef<Path>) -> bool {
    let value = path.as_ref().to_string_lossy().to_lowercase();
    VIRTUAL_ROOTS
        .iter()
        .any(|root| value == *root || value.strip_prefix(root).is_some_and(|rest| rest.starts_with('/')))
}

/// Content extraction exclusion (0.1 `collectors/content.py::excluded`).
pub fn is_excluded_content_path(path: impl AsRef<Path>) -> bool {
    is_virtual_fs_path(&path) || is_sensitive_path(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_basenames() {
        for path in [
            "/w/.env",
            "/w/.ENV",
            "/w/.env.local",
            "/w/certs/server.pem",
            "/w/tls.key",
            "/home/u/id_rsa",
            "/home/u/id_rsa.pub",
            "/home/u/id_ed25519",
            "/etc/shadow",
            "/etc/passwd",
            "/home/u/.netrc",
            "/w/cert.p12",
            "C:\\Users\\u\\.ssh\\config",
        ] {
            assert!(is_sensitive_path(path), "{path}");
        }
    }

    #[test]
    fn sensitive_terms_anywhere_in_path() {
        for path in [
            "/w/config/secrets.yaml",
            "/w/Credentials/aws.txt",
            "/w/password_reset.py",
            "/w/src/tokenizer.rs",
            "/home/u/client_secret/x.json",
        ] {
            assert!(is_sensitive_path(path), "{path}");
        }
    }

    #[test]
    fn sensitive_directories() {
        assert!(is_sensitive_path("/home/u/.ssh/config"));
        assert!(is_sensitive_path("/home/u/.gnupg/pubring.kbx"));
        assert!(is_sensitive_path("/home/u/.mozilla/firefox/x/places.sqlite"));
        assert!(is_sensitive_path("/home/u/.config/google-chrome/Default/Cookies"));
        assert!(is_sensitive_path("/home/u/.config/chromium/Default/History"));
        assert!(!is_sensitive_path("/home/u/.config/chromium-notes/readme.md"));
    }

    #[test]
    fn ordinary_paths_are_not_sensitive() {
        for path in [
            "/w/src/main.rs",
            "/w/README.md",
            "/w/.envrc",
            "/w/environment.ts",
            "/w/keyboard.py",
            "/w/monkey",
            "/w/docs/keys.md",
            "",
        ] {
            assert!(!is_sensitive_path(path), "{path}");
        }
    }

    #[test]
    fn virtual_roots() {
        assert!(is_virtual_fs_path("/proc"));
        assert!(is_virtual_fs_path("/proc/1/environ"));
        assert!(is_virtual_fs_path("/run/user/1000/x"));
        assert!(!is_virtual_fs_path("/procedures/x"));
        assert!(!is_virtual_fs_path("/home/u/run/x"));
        assert!(is_excluded_content_path("/sys/class"));
        assert!(is_excluded_content_path("/home/u/.ssh/id_rsa"));
        assert!(!is_excluded_content_path("/home/u/notes.md"));
    }
}
