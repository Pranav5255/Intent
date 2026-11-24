//! Ignore rules and capture eligibility for the filesystem watcher.

use std::path::{Component, Path, PathBuf};

/// Directory names never watched or reported (0.1 set plus common build/cache dirs).
pub const IGNORED_DIRS: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "node_modules",
    ".venv",
    "venv",
    "dist",
    "build",
    "__pycache__",
    "target",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".next",
    ".nuxt",
    ".direnv",
    ".gradle",
    ".cache",
];

/// 0.1 skipped files above 25 MB.
pub const MAX_FILE_BYTES: u64 = 25 * 1024 * 1024;

/// Kernel/virtual filesystems that are never workspace roots.
const VIRTUAL_ROOTS: &[&str] = &["/proc", "/sys", "/dev", "/run"];

pub fn is_ignored_dir_name(name: &str) -> bool {
    IGNORED_DIRS.contains(&name)
}

/// Editor swap/backup/lock files that only add noise (vim, emacs, JetBrains, LibreOffice).
pub fn is_ignored_file_name(name: &str) -> bool {
    name.ends_with(".swp")
        || name.ends_with(".swx")
        || name.ends_with(".swo")
        || name.ends_with('~')
        || name.starts_with(".#")
        || name.starts_with(".~lock.")
        || name.ends_with("___jb_tmp___")
        || name.ends_with("___jb_old___")
        || name == "4913"
}

/// True if any component of `path` below `root` is an ignored directory, or the
/// file name itself is an ignored editor artefact.
pub fn is_ignored_path(path: &Path, root: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else {
        return true;
    };
    let mut comps = rel.components().peekable();
    while let Some(c) = comps.next() {
        let Component::Normal(name) = c else {
            return true;
        };
        let name = name.to_string_lossy();
        let is_last = comps.peek().is_none();
        if is_last {
            if is_ignored_file_name(&name) {
                return true;
            }
        } else if is_ignored_dir_name(&name) {
            return true;
        }
    }
    false
}

/// Most specific root containing `path`.
pub fn owning_root<'a>(path: &Path, roots: &'a [PathBuf]) -> Option<&'a PathBuf> {
    roots.iter().filter(|r| path.starts_with(r)).max_by_key(|r| r.components().count())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootRejection {
    FilesystemRoot,
    Virtual,
    NotADirectory,
    Sensitive,
}

impl std::fmt::Display for RootRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::FilesystemRoot => "refusing to watch / recursively (use the home-directory option instead)",
            Self::Virtual => "virtual filesystem",
            Self::NotADirectory => "not a directory",
            Self::Sensitive => "sensitive path",
        })
    }
}

/// Expand `~`, make absolute, resolve symlinks and validate a workspace root.
pub fn normalize_root(
    raw: &Path,
    home: Option<&Path>,
    is_sensitive: &dyn Fn(&Path) -> bool,
) -> Result<PathBuf, RootRejection> {
    let expanded = match (raw.strip_prefix("~"), home) {
        (Ok(rest), Some(h)) => h.join(rest),
        _ => raw.to_path_buf(),
    };
    let resolved = expanded.canonicalize().map_err(|_| RootRejection::NotADirectory)?;
    if resolved.parent().is_none() {
        return Err(RootRejection::FilesystemRoot);
    }
    if VIRTUAL_ROOTS.iter().any(|v| resolved.starts_with(v)) {
        return Err(RootRejection::Virtual);
    }
    if !resolved.is_dir() {
        return Err(RootRejection::NotADirectory);
    }
    if is_sensitive(&resolved) {
        return Err(RootRejection::Sensitive);
    }
    Ok(resolved)
}

/// A workspace root that does not exist (yet): `~` expanded, absolute, with `.` and
/// `..` removed. `None` if the path exists, is relative, or would be refused anyway.
pub fn missing_root(raw: &Path, home: Option<&Path>, is_sensitive: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    let expanded = match (raw.strip_prefix("~"), home) {
        (Ok(rest), Some(h)) => h.join(rest),
        _ => raw.to_path_buf(),
    };
    if !expanded.is_absolute() {
        return None;
    }
    let mut path = PathBuf::from("/");
    for component in expanded.components() {
        match component {
            Component::Normal(part) => path.push(part),
            Component::ParentDir => {
                path.pop();
            }
            _ => {}
        }
    }
    let gone = matches!(std::fs::symlink_metadata(&path), Err(e) if e.kind() == std::io::ErrorKind::NotFound);
    let refused = path.parent().is_none() || VIRTUAL_ROOTS.iter().any(|v| path.starts_with(v)) || is_sensitive(&path);
    (gone && !refused).then_some(path)
}

/// Nearest existing directory above `path`.
pub fn nearest_existing_parent(path: &Path) -> Option<PathBuf> {
    path.ancestors().skip(1).find(|dir| dir.is_dir()).map(Path::to_path_buf)
}

/// Full eligibility check for a changed path (touches the filesystem).
/// Symlinks, non-regular files, oversized files, ignored and sensitive paths are skipped.
pub fn should_capture(path: &Path, root: &Path, max_bytes: u64, is_sensitive: &dyn Fn(&Path) -> bool) -> bool {
    if is_ignored_path(path, root) || is_sensitive(path) {
        return false;
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) => meta.file_type().is_file() && meta.len() <= max_bytes,
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn never(_: &Path) -> bool {
        false
    }

    #[test]
    fn ignore_rules() {
        let root = Path::new("/w");
        assert!(!is_ignored_path(Path::new("/w/src/main.rs"), root));
        assert!(is_ignored_path(Path::new("/w/.git/HEAD"), root));
        assert!(is_ignored_path(Path::new("/w/a/node_modules/x/index.js"), root));
        assert!(is_ignored_path(Path::new("/w/target/debug/foo"), root));
        assert!(is_ignored_path(Path::new("/w/.venv/lib/site.py"), root));
        assert!(is_ignored_path(Path::new("/w/src/.main.rs.swp"), root));
        assert!(is_ignored_path(Path::new("/w/src/4913"), root));
        assert!(is_ignored_path(Path::new("/w/notes.md~"), root));
        assert!(is_ignored_path(Path::new("/elsewhere/a.txt"), root), "outside root");
        // A *file* named like an ignored dir is fine; ignored names only apply to directories.
        assert!(!is_ignored_path(Path::new("/w/docs/build"), root));
        // The root itself may live under an ignored-looking name.
        assert!(!is_ignored_path(Path::new("/home/u/build/proj/a.c"), Path::new("/home/u/build/proj")));
    }

    #[test]
    fn owning_root_prefers_most_specific() {
        let roots = vec![PathBuf::from("/home/u"), PathBuf::from("/home/u/proj")];
        assert_eq!(owning_root(Path::new("/home/u/proj/a.rs"), &roots), Some(&roots[1]));
        assert_eq!(owning_root(Path::new("/home/u/b.txt"), &roots), Some(&roots[0]));
        assert_eq!(owning_root(Path::new("/home/user2/b.txt"), &roots), None);
    }

    #[test]
    fn root_validation() {
        assert_eq!(normalize_root(Path::new("/"), None, &never), Err(RootRejection::FilesystemRoot));
        assert_eq!(normalize_root(Path::new("/definitely/missing"), None, &never), Err(RootRejection::NotADirectory));
        let dir = tempfile::tempdir().unwrap();
        let ok = normalize_root(dir.path(), None, &never).unwrap();
        assert!(ok.is_absolute());
        let tilde = normalize_root(Path::new("~"), Some(dir.path()), &never).unwrap();
        assert_eq!(tilde, ok);
        assert_eq!(normalize_root(dir.path(), None, &|_| true), Err(RootRejection::Sensitive));
        let file = dir.path().join("f");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(normalize_root(&file, None, &never), Err(RootRejection::NotADirectory));
    }

    #[test]
    fn missing_roots_are_lexical_and_refused_like_roots() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        assert_eq!(missing_root(&base.join("a/../b/./c"), None, &never), Some(base.join("b/c")));
        assert_eq!(missing_root(Path::new("~/gone"), Some(&base), &never), Some(base.join("gone")));
        assert_eq!(missing_root(&base, None, &never), None, "exists");
        assert_eq!(missing_root(Path::new("rel/x"), None, &never), None);
        assert_eq!(missing_root(Path::new("/proc/nope-intent"), None, &never), None);
        assert_eq!(missing_root(&base.join("s"), None, &|_| true), None);
        assert_eq!(nearest_existing_parent(&base.join("x/y/z")), Some(base.clone()));
    }

    #[test]
    fn capture_eligibility() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let f = root.join("a.txt");
        std::fs::write(&f, "hello").unwrap();
        assert!(should_capture(&f, &root, MAX_FILE_BYTES, &never));
        assert!(!should_capture(&f, &root, 2, &never), "too large");
        assert!(!should_capture(&f, &root, MAX_FILE_BYTES, &|p| p.ends_with("a.txt")), "sensitive");
        assert!(!should_capture(&root.join("missing"), &root, MAX_FILE_BYTES, &never));
        assert!(!should_capture(&root, &root, MAX_FILE_BYTES, &never), "directory");
        #[cfg(unix)]
        {
            let link = root.join("link.txt");
            std::os::unix::fs::symlink(&f, &link).unwrap();
            assert!(!should_capture(&link, &root, MAX_FILE_BYTES, &never), "symlink");
        }
    }
}
