//! In-memory config cache with change detection.
//!
//! 1.x re-read and re-parsed every config file on every request (including every
//! ingested event). [`ConfigStore`] keeps the parsed value and only reloads when the
//! file's modification stamp (mtime, size, inode) changes, so edits made by hand or by
//! another process are still picked up on the next call.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

use serde_json::Value;

use super::{
    ConfigError, DetailedCaptureConfig, DetailedCaptureUpdate, FilesystemCaptureConfig, JsonConfig, RetentionPolicy,
    WorkspacesConfig,
};
use crate::admit::AdmissionPolicy;
use crate::blocklist::Blocklist;
use crate::paths::Paths;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    modified: Option<SystemTime>,
    len: u64,
    inode: u64,
}

fn stamp(path: &Path) -> Result<Option<Stamp>, ConfigError> {
    match std::fs::metadata(path) {
        Ok(meta) => {
            #[cfg(unix)]
            let inode = std::os::unix::fs::MetadataExt::ino(&meta);
            #[cfg(not(unix))]
            let inode = 0;
            Ok(Some(Stamp { modified: meta.modified().ok(), len: meta.len(), inode }))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(ConfigError::Io(format!("cannot stat {}: {err}", path.display()))),
    }
}

struct Entry<T> {
    stamp: Option<Stamp>,
    value: Result<Arc<T>, ConfigError>,
}

struct Cached<T> {
    path: PathBuf,
    loader: fn(&Path) -> Result<T, ConfigError>,
    entry: Mutex<Option<Entry<T>>>,
}

impl<T> Cached<T> {
    fn new(path: PathBuf, loader: fn(&Path) -> Result<T, ConfigError>) -> Self {
        Cached { path, loader, entry: Mutex::new(None) }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Entry<T>>> {
        self.entry.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn get_locked(&self, guard: &mut MutexGuard<'_, Option<Entry<T>>>) -> Result<Arc<T>, ConfigError> {
        let current = stamp(&self.path)?;
        if let Some(entry) = guard.as_ref()
            && entry.stamp == current
        {
            return entry.value.clone();
        }
        let value = (self.loader)(&self.path).map(Arc::new);
        **guard = Some(Entry { stamp: current, value: value.clone() });
        value
    }

    fn get(&self) -> Result<Arc<T>, ConfigError> {
        let mut guard = self.lock();
        self.get_locked(&mut guard)
    }

    /// Read-modify-write under the cache lock so concurrent updates serialise.
    fn modify<F>(&self, f: F) -> Result<Arc<T>, ConfigError>
    where
        F: FnOnce(Result<Arc<T>, ConfigError>) -> Result<T, ConfigError>,
        T: JsonConfig,
    {
        let mut guard = self.lock();
        let current = self.get_locked(&mut guard);
        let next = f(current)?;
        let written = Arc::new(next.save(&self.path)?);
        *guard = Some(Entry { stamp: stamp(&self.path)?, value: Ok(written.clone()) });
        Ok(written)
    }

    fn invalidate(&self) {
        *self.lock() = None;
    }
}

/// Cached access to every capture config file.
///
/// Cheap to share (`Arc<ConfigStore>`); all methods take `&self`.
pub struct ConfigStore {
    detailed: Cached<DetailedCaptureConfig>,
    workspaces: Cached<WorkspacesConfig>,
    filesystem: Cached<FilesystemCaptureConfig>,
    retention: Cached<RetentionPolicy>,
    blocklist: Cached<Blocklist>,
}

impl ConfigStore {
    pub fn new(paths: &Paths) -> Self {
        ConfigStore {
            detailed: Cached::new(paths.detailed_capture_config(), DetailedCaptureConfig::load),
            workspaces: Cached::new(paths.workspaces_config(), WorkspacesConfig::load),
            filesystem: Cached::new(paths.filesystem_capture_config(), FilesystemCaptureConfig::load),
            retention: Cached::new(paths.retention_policy_config(), RetentionPolicy::load),
            blocklist: Cached::new(paths.blocked_domains_config(), Blocklist::load),
        }
    }

    /// Store rooted at the process environment's XDG paths.
    pub fn from_env() -> Self {
        Self::new(&Paths::from_env())
    }

    // ---- detailed capture -------------------------------------------------

    pub fn detailed_capture(&self) -> Result<Arc<DetailedCaptureConfig>, ConfigError> {
        self.detailed.get()
    }

    pub fn set_detailed_capture(
        &self,
        config: DetailedCaptureConfig,
    ) -> Result<Arc<DetailedCaptureConfig>, ConfigError> {
        self.detailed.modify(|_| Ok(config))
    }

    /// Apply a partial update atomically (load → apply → validate → write).
    pub fn update_detailed_capture(
        &self,
        update: &DetailedCaptureUpdate,
    ) -> Result<Arc<DetailedCaptureConfig>, ConfigError> {
        self.detailed.modify(|current| {
            let mut next = (*current?).clone();
            update.apply(&mut next);
            Ok(next)
        })
    }

    /// `GET /v1/detailed-capture/config` body.
    pub fn public_detailed_config(&self) -> Result<Value, ConfigError> {
        let config = self.detailed_capture()?;
        let retention =
            self.retention_policy().map(|policy| policy.public_summary()).unwrap_or_else(|_| "indefinite".into());
        Ok(config.public_config(&self.approved_workspaces(), &retention))
    }

    // ---- workspaces -------------------------------------------------------

    pub fn workspaces(&self) -> Result<Arc<WorkspacesConfig>, ConfigError> {
        self.workspaces.get()
    }

    pub fn set_workspaces(&self, config: WorkspacesConfig) -> Result<Arc<WorkspacesConfig>, ConfigError> {
        self.workspaces.modify(|_| Ok(config))
    }

    pub fn add_workspace(&self, path: &str) -> Result<Arc<WorkspacesConfig>, ConfigError> {
        self.workspaces.modify(|current| Ok(current?.with_added(path)))
    }

    pub fn remove_workspace(&self, path: &str) -> Result<Arc<WorkspacesConfig>, ConfigError> {
        self.workspaces.modify(|current| Ok(current?.with_removed(path)))
    }

    /// Approved workspaces; an unreadable/invalid file means none (1.x behaviour).
    pub fn approved_workspaces(&self) -> Vec<String> {
        self.workspaces().map(|config| config.workspaces.clone()).unwrap_or_default()
    }

    // ---- filesystem -------------------------------------------------------

    pub fn filesystem_capture(&self) -> Result<Arc<FilesystemCaptureConfig>, ConfigError> {
        self.filesystem.get()
    }

    pub fn set_filesystem_capture(
        &self,
        config: FilesystemCaptureConfig,
    ) -> Result<Arc<FilesystemCaptureConfig>, ConfigError> {
        self.filesystem.modify(|_| Ok(config))
    }

    // ---- retention --------------------------------------------------------

    pub fn retention_policy(&self) -> Result<Arc<RetentionPolicy>, ConfigError> {
        self.retention.get()
    }

    pub fn set_retention_policy(&self, policy: RetentionPolicy) -> Result<Arc<RetentionPolicy>, ConfigError> {
        policy.validate()?;
        self.retention.modify(|_| Ok(policy))
    }

    // ---- blocklist --------------------------------------------------------

    /// Blocked domains (read-only; the YAML file is user-edited).
    pub fn blocklist(&self) -> Result<Arc<Blocklist>, ConfigError> {
        self.blocklist.get()
    }

    // ---- ingest -----------------------------------------------------------

    /// Snapshot of everything [`crate::admit::admit`] needs.
    pub fn admission_policy(&self) -> Result<AdmissionPolicy, ConfigError> {
        Ok(AdmissionPolicy {
            detailed: self.detailed_capture()?,
            approved_workspaces: Arc::new(self.approved_workspaces()),
            blocklist: self.blocklist()?,
        })
    }

    /// Drop every cached value (next access re-reads from disk).
    pub fn invalidate_all(&self) {
        self.detailed.invalidate();
        self.workspaces.invalidate();
        self.filesystem.invalidate();
        self.retention.invalidate();
        self.blocklist.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DetailedCaptureBrowserUpdate, DetailedKind};
    use crate::paths::{ENV_DETAILED_CAPTURE_CONFIG, ENV_WORKSPACES_CONFIG};
    use serde_json::json;
    use std::collections::HashMap;
    use std::ffi::OsString;

    fn store_in(dir: &Path, extra: &[(&str, String)]) -> (ConfigStore, Paths) {
        let mut vars: HashMap<String, String> = HashMap::new();
        vars.insert("HOME".into(), dir.join("home").to_string_lossy().into_owned());
        vars.insert("XDG_CONFIG_HOME".into(), dir.join("cfg").to_string_lossy().into_owned());
        for (key, value) in extra {
            vars.insert(key.to_string(), value.clone());
        }
        let paths = Paths::from_lookup(move |key| vars.get(key).map(OsString::from));
        (ConfigStore::new(&paths), paths)
    }

    #[test]
    fn missing_files_yield_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let (store, _) = store_in(dir.path(), &[]);
        assert_eq!(*store.detailed_capture().unwrap(), DetailedCaptureConfig::default());
        assert!(store.workspaces().unwrap().workspaces.is_empty());
        assert!(!store.filesystem_capture().unwrap().all_accessible);
        assert_eq!(*store.retention_policy().unwrap(), RetentionPolicy::default());
        assert!(store.blocklist().unwrap().is_empty());
        assert_eq!(store.public_detailed_config().unwrap()["retention"], "indefinite");
    }

    #[test]
    fn round_trips_every_config_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let (store, paths) = store_in(dir.path(), &[]);

        let mut detailed = DetailedCaptureConfig::default();
        detailed.set_enabled(DetailedKind::Editor, true);
        store.set_detailed_capture(detailed.clone()).unwrap();
        let workspace = crate::paths::resolve_lenient(dir.path()).join("w").to_string_lossy().into_owned();
        store.set_workspaces(WorkspacesConfig { workspaces: vec![workspace.clone()] }).unwrap();
        store.set_filesystem_capture(FilesystemCaptureConfig { all_accessible: true }).unwrap();
        store.set_retention_policy(RetentionPolicy { metadata_days: Some(30), detailed_days: Some(7) }).unwrap();

        // A fresh store reads exactly what was written.
        let fresh = ConfigStore::new(&paths);
        assert_eq!(*fresh.detailed_capture().unwrap(), detailed);
        assert_eq!(fresh.workspaces().unwrap().workspaces, vec![workspace.clone()]);
        assert!(fresh.filesystem_capture().unwrap().all_accessible);
        assert_eq!(fresh.retention_policy().unwrap().public_summary(), "metadata:30d, detailed:7d");

        let text = std::fs::read_to_string(paths.retention_policy_config()).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), json!({"metadata_days": 30, "detailed_days": 7}));
        assert!(text.ends_with("}\n"));
        let public = fresh.public_detailed_config().unwrap();
        assert_eq!(public["approved_workspaces"], json!([workspace]));
        assert_eq!(public["retention"], "metadata:30d, detailed:7d");
        assert_eq!(public["editor"]["enabled"], true);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [paths.detailed_capture_config(), paths.workspaces_config(), paths.filesystem_capture_config()]
            {
                assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            }
        }
        let leftovers: Vec<_> = std::fs::read_dir(paths.config_dir())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn env_overrides_redirect_files() {
        let dir = tempfile::tempdir().unwrap();
        let detailed_path = dir.path().join("custom/detailed.json");
        let workspaces_path = dir.path().join("ws.json");
        let (store, paths) = store_in(
            dir.path(),
            &[
                (ENV_DETAILED_CAPTURE_CONFIG, detailed_path.to_string_lossy().into_owned()),
                (ENV_WORKSPACES_CONFIG, workspaces_path.to_string_lossy().into_owned()),
            ],
        );
        assert_eq!(paths.detailed_capture_config(), detailed_path);
        store
            .update_detailed_capture(&DetailedCaptureUpdate {
                browser: Some(DetailedCaptureBrowserUpdate { enabled: Some(true), context_enabled: None }),
                ..Default::default()
            })
            .unwrap();
        assert!(detailed_path.exists());
        store.add_workspace("/nonexistent-intent/proj").unwrap();
        assert!(workspaces_path.exists());
        assert!(!paths.config_dir().join("detailed-capture.json").exists());
        assert_eq!(store.remove_workspace("/nonexistent-intent/proj").unwrap().workspaces.len(), 0);
    }

    #[test]
    fn external_edits_are_picked_up() {
        let dir = tempfile::tempdir().unwrap();
        let (store, paths) = store_in(dir.path(), &[]);
        assert!(!store.detailed_capture().unwrap().browser.enabled);
        let first = store.detailed_capture().unwrap();
        assert!(Arc::ptr_eq(&first, &store.detailed_capture().unwrap()), "unchanged file must be served from cache");

        crate::atomic::write_json_atomic(&paths.detailed_capture_config(), &json!({"browser": {"enabled": true}}))
            .unwrap();
        assert!(store.detailed_capture().unwrap().browser.enabled);

        std::fs::write(paths.blocked_domains_config(), "blocked_domains:\n  - chase.com\n").unwrap();
        assert_eq!(store.blocklist().unwrap().patterns(), ["chase.com"]);
        std::fs::write(paths.blocked_domains_config(), "blocked_domains:\n  - chase.com\n  - bank.org\n").unwrap();
        assert_eq!(store.blocklist().unwrap().patterns(), ["chase.com", "bank.org"]);
        std::fs::remove_file(paths.blocked_domains_config()).unwrap();
        assert!(store.blocklist().unwrap().is_empty());
    }

    #[test]
    fn invalid_files_report_errors_and_updates_validate() {
        let dir = tempfile::tempdir().unwrap();
        let (store, paths) = store_in(dir.path(), &[]);
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(paths.detailed_capture_config(), "{not json").unwrap();
        assert_eq!(
            store.detailed_capture().unwrap_err().to_string(),
            format!("invalid detailed-capture config: {}", paths.detailed_capture_config().display())
        );
        std::fs::write(paths.workspaces_config(), "[]").unwrap();
        assert!(store.workspaces().is_err());
        assert!(store.approved_workspaces().is_empty());
        std::fs::write(paths.retention_policy_config(), "{\"metadata_days\": 0}").unwrap();
        assert!(store.public_detailed_config().unwrap_err().to_string().contains("invalid detailed-capture config"));

        std::fs::remove_file(paths.detailed_capture_config()).unwrap();
        assert_eq!(store.public_detailed_config().unwrap()["retention"], "indefinite");

        let bad = DetailedCaptureUpdate {
            editor: Some(crate::config::DetailedCaptureEditorUpdate {
                enabled: None,
                excluded_patterns: Some(vec![String::new()]),
            }),
            ..Default::default()
        };
        assert!(store.update_detailed_capture(&bad).is_err());
        assert!(!paths.detailed_capture_config().exists());
        assert!(store.set_retention_policy(RetentionPolicy { metadata_days: Some(0), detailed_days: None }).is_err());
    }

    #[test]
    fn admission_policy_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let (store, paths) = store_in(dir.path(), &[]);
        std::fs::create_dir_all(paths.config_dir()).unwrap();
        std::fs::write(paths.blocked_domains_config(), "blocked_domains: [chase.com]\n").unwrap();
        store.add_workspace("/nonexistent-intent/w").unwrap();
        let policy = store.admission_policy().unwrap();
        assert_eq!(policy.blocklist.patterns(), ["chase.com"]);
        assert_eq!(*policy.approved_workspaces, vec!["/nonexistent-intent/w".to_string()]);
        store.invalidate_all();
        assert!(store.admission_policy().is_ok());
    }
}
