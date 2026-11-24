//! Crash-safe file replacement: tempfile in the target directory + fsync + rename.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use serde::Serialize;

/// Create `dir` (and parents) if missing; new directories are 0700 on Unix.
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    if dir.as_os_str().is_empty() || dir.is_dir() {
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Atomically replace `path` with `bytes`. The file is created with mode 0600 on Unix
/// (the temporary file's mode survives the rename).
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    ensure_private_dir(parent)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut temporary = tempfile::Builder::new().prefix(&format!(".{name}.")).suffix(".tmp").tempfile_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|err| err.error)?;
    // Best effort: make the rename itself durable.
    #[cfg(unix)]
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

/// Atomically write `value` as 2-space indented JSON plus a trailing newline
/// (the 0.1 on-disk format).
pub fn write_json_atomic<T: Serialize + ?Sized>(path: &Path, value: &T) -> io::Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    bytes.push(b'\n');
    write_atomic(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_content_and_leaves_no_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/deeper/config.json");
        write_json_atomic(&path, &serde_json::json!({"a": 1})).unwrap();
        write_json_atomic(&path, &serde_json::json!({"a": 2})).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text, "{\n  \"a\": 2\n}\n");
        let entries: Vec<_> = fs::read_dir(path.parent().unwrap()).unwrap().collect();
        assert_eq!(entries.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn files_are_0600_and_new_dirs_0700() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("intent/token");
        write_atomic(&path, b"secret").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let dir_mode = fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
    }
}
