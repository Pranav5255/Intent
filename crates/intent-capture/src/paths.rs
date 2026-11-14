//! The one XDG path resolver shared by every Intent component.
//!
//! 1.x resolved `XDG_CONFIG_HOME` separately in six modules (and treated an empty or
//! relative value as a relative path). Here the XDG base directories are resolved
//! once, following the XDG Base Directory spec: empty or relative values are ignored.
//! The 1.x `INTENT_*_CONFIG` per-file overrides keep working.

use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

pub const ENV_DETAILED_CAPTURE_CONFIG: &str = "INTENT_DETAILED_CAPTURE_CONFIG";
pub const ENV_WORKSPACES_CONFIG: &str = "INTENT_WORKSPACES_CONFIG";
pub const ENV_RETENTION_POLICY_CONFIG: &str = "INTENT_RETENTION_POLICY_CONFIG";
pub const ENV_BLOCKED_DOMAINS_CONFIG: &str = "INTENT_BLOCKED_DOMAINS_CONFIG";
/// New in 2.0 (1.x had no override for this file).
pub const ENV_FILESYSTEM_CAPTURE_CONFIG: &str = "INTENT_FILESYSTEM_CAPTURE_CONFIG";
pub const ENV_CONFIG_DIR: &str = "INTENT_CONFIG_DIR";
/// 1.x override for the capture database (`events.db`); used by the importer.
pub const ENV_LEGACY_DATABASE: &str = "INTENT_DATABASE";
/// 1.x override for `llm.env`; used by the one-time LLM settings import.
pub const ENV_LEGACY_LLM_CONFIG: &str = "INTENT_LLM_CONFIG";

pub const DETAILED_CAPTURE_FILE: &str = "detailed-capture.json";
pub const WORKSPACES_FILE: &str = "workspaces.json";
pub const FILESYSTEM_CAPTURE_FILE: &str = "filesystem-capture.json";
pub const RETENTION_POLICY_FILE: &str = "retention-policy.json";
pub const BLOCKED_DOMAINS_FILE: &str = "blocked-domains.yaml";
pub const LLM_CONFIG_FILE: &str = "llm.toml";
pub const DATABASE_FILE: &str = "intent.db";
pub const TOKEN_FILE: &str = "token";

/// Resolved Intent directories and file locations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    home: PathBuf,
    config_home: PathBuf,
    data_home: PathBuf,
    runtime_home: Option<PathBuf>,
    config_dir_override: Option<PathBuf>,
    detailed_capture_override: Option<PathBuf>,
    workspaces_override: Option<PathBuf>,
    retention_override: Option<PathBuf>,
    blocked_domains_override: Option<PathBuf>,
    filesystem_capture_override: Option<PathBuf>,
    legacy_database_override: Option<PathBuf>,
    legacy_llm_override: Option<PathBuf>,
}

impl Paths {
    /// Resolve from the process environment.
    pub fn from_env() -> Self {
        Self::from_lookup(|key| std::env::var_os(key))
    }

    /// Resolve from an arbitrary environment lookup (used by tests; avoids `set_var`).
    pub fn from_lookup<F>(lookup: F) -> Self
    where
        F: Fn(&str) -> Option<OsString>,
    {
        let non_empty = |key: &str| lookup(key).filter(|value| !value.to_string_lossy().trim().is_empty());
        let home = non_empty("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
        // XDG spec: relative or empty values are invalid and must be ignored.
        let xdg = |key: &str| non_empty(key).map(PathBuf::from).filter(|path| path.is_absolute());
        let over = |key: &str| non_empty(key).map(|value| expand_user_with(Path::new(&value), &home));
        Paths {
            config_home: xdg("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")),
            data_home: xdg("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local").join("share")),
            runtime_home: xdg("XDG_RUNTIME_DIR"),
            config_dir_override: over(ENV_CONFIG_DIR),
            detailed_capture_override: over(ENV_DETAILED_CAPTURE_CONFIG),
            workspaces_override: over(ENV_WORKSPACES_CONFIG),
            retention_override: over(ENV_RETENTION_POLICY_CONFIG),
            blocked_domains_override: over(ENV_BLOCKED_DOMAINS_CONFIG),
            filesystem_capture_override: over(ENV_FILESYSTEM_CAPTURE_CONFIG),
            legacy_database_override: over(ENV_LEGACY_DATABASE),
            legacy_llm_override: over(ENV_LEGACY_LLM_CONFIG),
            home,
        }
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    /// `$INTENT_CONFIG_DIR` or `$XDG_CONFIG_HOME/intent`.
    pub fn config_dir(&self) -> PathBuf {
        self.config_dir_override.clone().unwrap_or_else(|| self.config_home.join("intent"))
    }

    /// `$XDG_DATA_HOME/intent`.
    pub fn data_dir(&self) -> PathBuf {
        self.data_home.join("intent")
    }

    /// `$XDG_RUNTIME_DIR/intent`, if a runtime dir is available.
    pub fn runtime_dir(&self) -> Option<PathBuf> {
        self.runtime_home.as_ref().map(|dir| dir.join("intent"))
    }

    pub fn detailed_capture_config(&self) -> PathBuf {
        self.file(&self.detailed_capture_override, DETAILED_CAPTURE_FILE)
    }

    pub fn workspaces_config(&self) -> PathBuf {
        self.file(&self.workspaces_override, WORKSPACES_FILE)
    }

    pub fn filesystem_capture_config(&self) -> PathBuf {
        self.file(&self.filesystem_capture_override, FILESYSTEM_CAPTURE_FILE)
    }

    pub fn retention_policy_config(&self) -> PathBuf {
        self.file(&self.retention_override, RETENTION_POLICY_FILE)
    }

    pub fn blocked_domains_config(&self) -> PathBuf {
        self.file(&self.blocked_domains_override, BLOCKED_DOMAINS_FILE)
    }

    /// 2.0 LLM settings (`llm.toml`, 0600).
    pub fn llm_config(&self) -> PathBuf {
        self.config_dir().join(LLM_CONFIG_FILE)
    }

    /// 1.x `llm.env`, imported once.
    pub fn legacy_llm_env(&self) -> PathBuf {
        self.file(&self.legacy_llm_override, "llm.env")
    }

    /// The single 2.0 SQLite database.
    pub fn database(&self) -> PathBuf {
        self.data_dir().join(DATABASE_FILE)
    }

    /// 1.x capture database (`$INTENT_DATABASE` or `$XDG_DATA_HOME/intent/events.db`).
    pub fn legacy_events_db(&self) -> PathBuf {
        self.legacy_database_override.clone().unwrap_or_else(|| self.data_dir().join("events.db"))
    }

    /// Log directory (`$XDG_DATA_HOME/intent/logs`).
    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir().join("logs")
    }

    /// API bearer token: `$XDG_RUNTIME_DIR/intent/token`, falling back to the data dir.
    pub fn token(&self) -> PathBuf {
        self.runtime_dir().unwrap_or_else(|| self.data_dir()).join(TOKEN_FILE)
    }

    fn file(&self, override_path: &Option<PathBuf>, name: &str) -> PathBuf {
        override_path.clone().unwrap_or_else(|| self.config_dir().join(name))
    }
}

/// Expand a leading `~` / `~/` using `$HOME` (Python `Path.expanduser`).
pub fn expand_user(path: &Path) -> PathBuf {
    let home = std::env::var_os("HOME").filter(|h| !h.is_empty()).map(PathBuf::from);
    match home {
        Some(home) => expand_user_with(path, &home),
        None => path.to_path_buf(),
    }
}

fn expand_user_with(path: &Path, home: &Path) -> PathBuf {
    let mut components = path.components();
    match components.next() {
        Some(Component::Normal(first)) if first == OsStr::new("~") => home.join(components.as_path()),
        _ => path.to_path_buf(),
    }
}

/// Non-strict `realpath`: absolutise against the cwd, resolve symlinks for the parts
/// that exist and normalise `.`/`..` (Python `Path.resolve(strict=False)`).
pub fn resolve_lenient(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map(|cwd| cwd.join(path)).unwrap_or_else(|_| path.to_path_buf())
    };
    let mut pending: VecDeque<OsString> = VecDeque::new();
    push_components(&mut pending, &absolute, false);
    let mut out = PathBuf::from("/");
    let mut hops = 0usize;
    while let Some(part) = pending.pop_front() {
        if part == ".." {
            out.pop();
            continue;
        }
        out.push(&part);
        if hops < 40
            && let Ok(target) = std::fs::read_link(&out)
        {
            hops += 1;
            out.pop();
            if target.is_absolute() {
                out = PathBuf::from("/");
            }
            push_components(&mut pending, &target, true);
        }
    }
    out
}

/// `expand_user` followed by `resolve_lenient`, rendered as a string.
pub fn resolve_user_path(raw: &str) -> String {
    resolve_lenient(&expand_user(Path::new(raw))).to_string_lossy().into_owned()
}

fn push_components(queue: &mut VecDeque<OsString>, path: &Path, front: bool) {
    let parts: Vec<OsString> = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_os_string()),
            Component::ParentDir => Some(OsString::from("..")),
            _ => None,
        })
        .collect();
    if front {
        for part in parts.into_iter().rev() {
            queue.push_front(part);
        }
    } else {
        queue.extend(parts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn paths(vars: &[(&str, &str)]) -> Paths {
        let map: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Paths::from_lookup(move |key| map.get(key).map(OsString::from))
    }

    #[test]
    fn defaults_follow_home() {
        let p = paths(&[("HOME", "/home/u")]);
        assert_eq!(p.config_dir(), PathBuf::from("/home/u/.config/intent"));
        assert_eq!(p.detailed_capture_config(), PathBuf::from("/home/u/.config/intent/detailed-capture.json"));
        assert_eq!(p.workspaces_config(), PathBuf::from("/home/u/.config/intent/workspaces.json"));
        assert_eq!(p.filesystem_capture_config(), PathBuf::from("/home/u/.config/intent/filesystem-capture.json"));
        assert_eq!(p.retention_policy_config(), PathBuf::from("/home/u/.config/intent/retention-policy.json"));
        assert_eq!(p.blocked_domains_config(), PathBuf::from("/home/u/.config/intent/blocked-domains.yaml"));
        assert_eq!(p.database(), PathBuf::from("/home/u/.local/share/intent/intent.db"));
        assert_eq!(p.legacy_events_db(), PathBuf::from("/home/u/.local/share/intent/events.db"));
        assert_eq!(p.token(), PathBuf::from("/home/u/.local/share/intent/token"));
        assert_eq!(p.llm_config(), PathBuf::from("/home/u/.config/intent/llm.toml"));
        assert_eq!(p.runtime_dir(), None);
    }

    #[test]
    fn xdg_dirs_are_honoured() {
        let p = paths(&[
            ("HOME", "/home/u"),
            ("XDG_CONFIG_HOME", "/cfg"),
            ("XDG_DATA_HOME", "/data"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
        ]);
        assert_eq!(p.blocked_domains_config(), PathBuf::from("/cfg/intent/blocked-domains.yaml"));
        assert_eq!(p.database(), PathBuf::from("/data/intent/intent.db"));
        assert_eq!(p.token(), PathBuf::from("/run/user/1000/intent/token"));
        assert_eq!(p.logs_dir(), PathBuf::from("/data/intent/logs"));
    }

    #[test]
    fn empty_or_relative_xdg_values_are_ignored() {
        let p = paths(&[("HOME", "/home/u"), ("XDG_CONFIG_HOME", ""), ("XDG_DATA_HOME", "rel/data")]);
        assert_eq!(p.config_dir(), PathBuf::from("/home/u/.config/intent"));
        assert_eq!(p.data_dir(), PathBuf::from("/home/u/.local/share/intent"));
    }

    #[test]
    fn per_file_overrides_win_and_expand_tilde() {
        let p = paths(&[
            ("HOME", "/home/u"),
            ("XDG_CONFIG_HOME", "/cfg"),
            (ENV_DETAILED_CAPTURE_CONFIG, "~/d.json"),
            (ENV_WORKSPACES_CONFIG, "/w.json"),
            (ENV_RETENTION_POLICY_CONFIG, "/r.json"),
            (ENV_BLOCKED_DOMAINS_CONFIG, "/b.yaml"),
            (ENV_FILESYSTEM_CAPTURE_CONFIG, "/f.json"),
            (ENV_LEGACY_DATABASE, "~/old.db"),
        ]);
        assert_eq!(p.detailed_capture_config(), PathBuf::from("/home/u/d.json"));
        assert_eq!(p.workspaces_config(), PathBuf::from("/w.json"));
        assert_eq!(p.retention_policy_config(), PathBuf::from("/r.json"));
        assert_eq!(p.blocked_domains_config(), PathBuf::from("/b.yaml"));
        assert_eq!(p.filesystem_capture_config(), PathBuf::from("/f.json"));
        assert_eq!(p.legacy_events_db(), PathBuf::from("/home/u/old.db"));
    }

    #[test]
    fn config_dir_override_moves_all_default_files() {
        let p = paths(&[("HOME", "/home/u"), (ENV_CONFIG_DIR, "/etc/intent-test")]);
        assert_eq!(p.workspaces_config(), PathBuf::from("/etc/intent-test/workspaces.json"));
        assert_eq!(p.legacy_llm_env(), PathBuf::from("/etc/intent-test/llm.env"));
    }

    #[test]
    fn expand_user_only_touches_leading_tilde() {
        let home = Path::new("/h");
        assert_eq!(expand_user_with(Path::new("~"), home), PathBuf::from("/h"));
        assert_eq!(expand_user_with(Path::new("~/a/b"), home), PathBuf::from("/h/a/b"));
        assert_eq!(expand_user_with(Path::new("/a/~"), home), PathBuf::from("/a/~"));
        assert_eq!(expand_user_with(Path::new("~x/a"), home), PathBuf::from("~x/a"));
    }

    #[test]
    fn resolve_lenient_normalises_missing_paths() {
        assert_eq!(
            resolve_lenient(Path::new("/nonexistent-intent/a/../b/./c")),
            PathBuf::from("/nonexistent-intent/b/c")
        );
        assert_eq!(resolve_lenient(Path::new("/../..")), PathBuf::from("/"));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_lenient_follows_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let real = resolve_lenient(dir.path());
        std::fs::create_dir(real.join("target")).unwrap();
        std::os::unix::fs::symlink(real.join("target"), real.join("link")).unwrap();
        assert_eq!(resolve_lenient(&real.join("link/missing.txt")), real.join("target/missing.txt"));
        std::os::unix::fs::symlink("target", real.join("rel")).unwrap();
        assert_eq!(resolve_lenient(&real.join("rel/../x")), real.join("x"));
    }
}
