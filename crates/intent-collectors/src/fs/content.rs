//! Bounded local text extraction for observed file changes (port of 1.x `collectors/content.py`).
//!
//! Never walks the disk: only a path the watcher just observed is examined.
//! Text files: first 64 KiB. PDFs via `pdftotext` (first 3 pages) and images via
//! `tesseract`, only when installed, each under a hard timeout.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use intent_types::Payload;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const MAX_READ_BYTES: usize = 64 * 1024;
pub const MAX_EXCERPT_CHARS: usize = 4_000;
const PDF_TIMEOUT: Duration = Duration::from_secs(5);
const OCR_TIMEOUT: Duration = Duration::from_secs(8);

const TEXT_SUFFIXES: &[&str] = &[
    "txt", "md", "rst", "csv", "json", "yaml", "yml", "toml", "ini", "py", "js", "ts", "tsx", "jsx", "html", "css",
    "sh", "xml", "rs", "go", "c", "h", "cpp", "hpp", "java", "kt", "rb", "sql", "tex", "org", "log",
];
const IMAGE_SUFFIXES: &[&str] = &["png", "jpg", "jpeg", "webp", "tiff", "bmp"];

/// External extractors discovered on `PATH` at collector start.
#[derive(Debug, Clone, Default)]
pub struct Tools {
    pub pdftotext: Option<PathBuf>,
    pub tesseract: Option<PathBuf>,
}

impl Tools {
    pub fn detect() -> Self {
        Self { pdftotext: which("pdftotext"), tesseract: which("tesseract") }
    }
}

pub fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(bin)).find(|p| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        }
        #[cfg(not(unix))]
        {
            p.is_file()
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentKind {
    Text,
    Pdf,
    Image,
}

impl ContentKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Pdf => "pdf",
            Self::Image => "image",
        }
    }
}

fn suffix(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

pub fn guess_mime(path: &Path) -> &'static str {
    match suffix(path).as_str() {
        "txt" | "log" => "text/plain",
        "md" => "text/markdown",
        "rst" => "text/x-rst",
        "csv" => "text/csv",
        "json" => "application/json",
        "yaml" | "yml" => "application/yaml",
        "toml" => "application/toml",
        "html" => "text/html",
        "css" => "text/css",
        "xml" => "application/xml",
        "js" | "jsx" => "text/javascript",
        "ts" | "tsx" => "text/x-typescript",
        "py" => "text/x-python",
        "sh" => "application/x-sh",
        "rs" => "text/x-rust",
        "c" | "h" => "text/x-c",
        "cpp" | "hpp" => "text/x-c++",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "tiff" => "image/tiff",
        "bmp" => "image/bmp",
        _ => "application/octet-stream",
    }
}

pub fn classify(path: &Path, tools: &Tools) -> Option<ContentKind> {
    let s = suffix(path);
    if TEXT_SUFFIXES.contains(&s.as_str()) || guess_mime(path).starts_with("text/") {
        Some(ContentKind::Text)
    } else if s == "pdf" && tools.pdftotext.is_some() {
        Some(ContentKind::Pdf)
    } else if IMAGE_SUFFIXES.contains(&s.as_str()) && tools.tesseract.is_some() {
        Some(ContentKind::Image)
    } else {
        None
    }
}

/// Collapse whitespace and cap at [`MAX_EXCERPT_CHARS`].
pub fn bounded(text: &str) -> String {
    let mut out = String::new();
    for (n, word) in text.split_whitespace().enumerate() {
        if n > 0 {
            out.push(' ');
        }
        out.push_str(word);
        if out.chars().count() >= MAX_EXCERPT_CHARS {
            break;
        }
    }
    match out.char_indices().nth(MAX_EXCERPT_CHARS) {
        Some((i, _)) => out[..i].to_owned(),
        None => out,
    }
}

fn read_head(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(8 * 1024);
    std::fs::File::open(path)?.take(MAX_READ_BYTES as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

async fn command_output(program: &Path, args: &[&std::ffi::OsStr], timeout: Duration) -> Option<String> {
    let child = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let out = tokio::time::timeout(timeout, child.wait_with_output()).await.ok()?.ok()?;
    if !out.status.success() || out.stdout.is_empty() {
        return None;
    }
    let text = bounded(&String::from_utf8_lossy(&out.stdout));
    (!text.is_empty()).then_some(text)
}

/// Extract a bounded excerpt. Returns the 1.x `file_content` payload
/// (`path, kind, mime, size_bytes, sha256, excerpt`) without `workspace`.
/// Blocking file reads run on tokio's blocking pool; callers bound concurrency.
pub async fn extract(path: PathBuf, max_file_bytes: u64, tools: &Tools) -> Option<Payload> {
    let kind = classify(&path, tools)?;
    let p = path.clone();
    let (size, head) = tokio::task::spawn_blocking(move || {
        let meta = std::fs::symlink_metadata(&p).ok()?;
        if !meta.file_type().is_file() || meta.len() > max_file_bytes {
            return None;
        }
        Some((meta.len(), read_head(&p).ok()?))
    })
    .await
    .ok()??;

    let excerpt = match kind {
        ContentKind::Text => bounded(&String::from_utf8_lossy(&head)),
        ContentKind::Pdf => {
            let bin = tools.pdftotext.as_deref()?;
            let args = ["-f".as_ref(), "1".as_ref(), "-l".as_ref(), "3".as_ref(), path.as_os_str(), "-".as_ref()];
            command_output(bin, &args, PDF_TIMEOUT).await?
        }
        ContentKind::Image => {
            let bin = tools.tesseract.as_deref()?;
            command_output(bin, &[path.as_os_str(), "stdout".as_ref()], OCR_TIMEOUT).await?
        }
    };
    if excerpt.is_empty() {
        return None;
    }
    let digest = Sha256::digest(&head);
    let mut payload = Payload::new();
    payload.insert("path".into(), Value::String(path.to_string_lossy().into_owned()));
    payload.insert("kind".into(), Value::String(kind.as_str().into()));
    payload.insert("mime".into(), Value::String(guess_mime(&path).into()));
    payload.insert("size_bytes".into(), Value::from(size));
    payload.insert("sha256".into(), Value::String(format!("{digest:x}")));
    payload.insert("excerpt".into(), Value::String(excerpt));
    Some(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_is_collapsed_and_bounded() {
        assert_eq!(bounded("  a\n\n b\tc  "), "a b c");
        let long = "word ".repeat(5_000);
        assert_eq!(bounded(&long).chars().count(), MAX_EXCERPT_CHARS);
    }

    #[test]
    fn classification_respects_installed_tools() {
        let none = Tools::default();
        assert_eq!(classify(Path::new("a.MD"), &none), Some(ContentKind::Text));
        assert_eq!(classify(Path::new("a.pdf"), &none), None);
        assert_eq!(classify(Path::new("a.png"), &none), None);
        assert_eq!(classify(Path::new("a.bin"), &none), None);
        let all = Tools { pdftotext: Some("/usr/bin/pdftotext".into()), tesseract: Some("/usr/bin/tesseract".into()) };
        assert_eq!(classify(Path::new("a.pdf"), &all), Some(ContentKind::Pdf));
        assert_eq!(classify(Path::new("a.jpeg"), &all), Some(ContentKind::Image));
    }

    #[tokio::test]
    async fn text_extraction_payload() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("notes.md");
        std::fs::write(&f, "# Title\n\nsome   body text\n").unwrap();
        let p = extract(f.clone(), 1024, &Tools::default()).await.unwrap();
        assert_eq!(p["kind"], "text");
        assert_eq!(p["mime"], "text/markdown");
        assert_eq!(p["excerpt"], "# Title some body text");
        assert_eq!(p["size_bytes"], 26);
        assert_eq!(p["sha256"].as_str().unwrap().len(), 64);
        assert!(extract(f, 4, &Tools::default()).await.is_none(), "over size cap");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn external_tool_timeout_is_enforced() {
        let started = std::time::Instant::now();
        let out = command_output(Path::new("/bin/sleep"), &["5".as_ref()], Duration::from_millis(100)).await;
        assert!(out.is_none());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
