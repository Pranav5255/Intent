//! Server-side final redaction, including shell commands, so a bypassed or
//! outdated client-side hook can't store secrets.

use std::sync::LazyLock;

use intent_types::RawEvent;
use regex::Regex;
use serde_json::Value;

pub use crate::url::sanitize_url;

/// Replacement for a document change containing a secret.
pub const REDACTED_TEXT: &str = "[redacted]";
/// Replacement for a shell command / shell argument (1.x sender format).
pub const REDACTED_COMMAND: &str = "<redacted>";
/// Title stored for a private-browsing window.
pub const PRIVATE_WINDOW_TITLE: &str = "Private window";
/// Commands longer than this (in characters) are fully redacted.
pub const MAX_COMMAND_LENGTH: usize = 500;

static PRIVATE_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----").expect("valid regex"));
/// A secret-looking name followed by `:` or `=`. The name may carry other words
/// joined by `_` or `-` (`DB_PASSWORD`, `STRIPE_SECRET_KEY`), because `\b` does not
/// split on `_`. A quote may close the name (`"api_key": ...`).
static SENSITIVE_ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)(?:^|[^A-Za-z0-9_-])(?:[A-Za-z0-9_-]*[_-])?(?:password|passwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|authorization)(?:[_-][A-Za-z0-9_-]*)?["']?\s*[:=]"#,
    )
    .expect("valid regex")
});
/// Well-known token formats: GitHub, Slack, JWT, OpenAI, Stripe, AWS access key
/// IDs, Google API keys and GitLab.
static TOKEN_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"\b(?:gh[pous]_[A-Za-z0-9_]+|github_pat_[A-Za-z0-9_]+|xox[baprs]-[A-Za-z0-9-]+|xapp-[A-Za-z0-9-]+",
        r"|eyJ[A-Za-z0-9_-]+\.|sk-[A-Za-z0-9_-]{20,}|[sr]k_(?:live|test)_[A-Za-z0-9]{8,}",
        r"|AKIA[0-9A-Z]{16}\b|AIza[0-9A-Za-z_-]{35}|glpat-[A-Za-z0-9_-]{20,})",
    ))
    .expect("valid regex")
});
/// Window-title markers of private browsing: Firefox, Chrome/Chromium, Edge, Brave.
static PRIVATE_WINDOW: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)private browsing|\bincognito\b|\binprivate\b|\bbrave\b.*\bprivate\b|\(private\)")
        .expect("valid regex")
});

/// Commands that look like they carry credentials or run with elevated rights.
static SENSITIVE_COMMAND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:password|passwd|secret|api[_-]?key|access[_-]?key|private[_-]?key|token|bearer|credential|authorization|ssh\b|gpg\b|sudo\b)",
    )
    .expect("valid regex")
});
/// `scheme://userinfo@` — credentials embedded in any URL.
static URL_USERINFO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b([a-z][a-z0-9+.\-]*://)[^\s/?#@]+@").expect("valid regex"));
/// `NAME=value` shell assignments (inline env vars, `export`, `env`).
static ENV_ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(^|[\s;&|(])([A-Za-z_][A-Za-z0-9_]*)=("[^"]*"|'[^']*'|[^\s;&|)]*)"#).expect("valid regex")
});

/// Error for events that must not be stored at all (HTTP 422 in 1.x).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct RedactionError {
    pub message: String,
}

/// `true` when text contains a private key, a `secret=`-style assignment or a
/// well-known token prefix.
pub fn should_redact_text(text: &str) -> bool {
    PRIVATE_KEY.is_match(text) || SENSITIVE_ASSIGNMENT.is_match(text) || TOKEN_PREFIX.is_match(text)
}

/// Return a copy of `event` that is safe to store.
///
/// - `vscode/document_change`: change texts with secrets become `[redacted]`.
/// - `firefox/*`: `url` keeps only scheme/host/port/path (non-HTTP(S) URLs reject the
///   event); `target.href` is sanitised or dropped; a secret-bearing `context` is dropped.
/// - `shell/command`: `cmd` is redacted with [`redact_shell_command`] (new in 2.0).
/// - `filesystem/file_content`: an excerpt with a secret becomes `[redacted]`. The
///   whole excerpt goes, not one line: a private key spans many lines, and the
///   lines around a secret often belong to it.
/// - `linux/app_focus`: a private-browsing window keeps its app but its title
///   becomes [`PRIVATE_WINDOW_TITLE`]. A title with a secret becomes `[redacted]`,
///   and password arguments in a title (terminals show the running command) are
///   masked. Collector events take this path too (`intentd` ingest).
pub fn redact_event(mut event: RawEvent) -> Result<RawEvent, RedactionError> {
    let payload = &mut event.payload;
    match event.source.as_str() {
        "vscode" if event.kind == "document_change" => {
            if let Some(Value::Array(changes)) = payload.get_mut("changes") {
                for change in changes.iter_mut().filter_map(Value::as_object_mut) {
                    let secret = matches!(change.get("text"), Some(Value::String(text)) if should_redact_text(text));
                    if secret {
                        change.insert("text".into(), Value::String(REDACTED_TEXT.into()));
                        change.insert("redacted".into(), Value::Bool(true));
                    }
                }
            }
        }
        "firefox" => {
            if let Some(url) = payload.get("url").and_then(Value::as_str) {
                let clean = sanitize_url(url).ok_or_else(|| RedactionError {
                    message: "firefox event url must be an http or https URL".into(),
                })?;
                payload.insert("url".into(), Value::String(clean));
            }
            if let Some(Value::Object(target)) = payload.get_mut("target")
                && let Some(href) = target.get("href").and_then(Value::as_str).map(str::to_owned)
            {
                match sanitize_url(&href) {
                    Some(clean) => {
                        target.insert("href".into(), Value::String(clean));
                    }
                    None => {
                        target.remove("href");
                    }
                }
            }
            let secret_context = matches!(
                payload.get("context").and_then(|context| context.get("text_excerpt")),
                Some(Value::String(text)) if should_redact_text(text)
            );
            if secret_context {
                payload.remove("context");
            }
        }
        "shell" if event.kind == "command" => {
            if let Some(clean) = payload.get("cmd").and_then(Value::as_str).map(redact_shell_command) {
                payload.insert("cmd".into(), Value::String(clean));
            }
        }
        "filesystem" if event.kind == "file_content" => {
            let secret = matches!(payload.get("excerpt"), Some(Value::String(text)) if should_redact_text(text));
            if secret {
                payload.insert("excerpt".into(), Value::String(REDACTED_TEXT.into()));
            }
        }
        "linux" if event.kind == "app_focus" => {
            if let Some(title) = payload.get("title").and_then(Value::as_str) {
                let clean = redact_window_title(title);
                if clean != title {
                    payload.insert("title".into(), Value::String(clean));
                }
            }
        }
        _ => {}
    }
    Ok(event)
}

/// Make a window title safe to store (see [`redact_event`]).
pub fn redact_window_title(title: &str) -> String {
    if PRIVATE_WINDOW.is_match(title) {
        PRIVATE_WINDOW_TITLE.to_string()
    } else if should_redact_text(title) {
        REDACTED_TEXT.to_string()
    } else {
        mask_inline_secrets(title)
    }
}

/// Redact a shell command line.
///
/// Fully replaced by `<redacted>` (1.x sender rules) when it is longer than
/// [`MAX_COMMAND_LENGTH`] characters or mentions password/secret/token/API key/
/// bearer/credential, or `ssh`/`gpg`/`sudo`, or contains a private key or known
/// token prefix. Otherwise individual secrets are masked in place so the command
/// family survives for intent inference:
///
/// - URL credentials: `https://user:pw@host` → `https://<redacted>@host`
/// - secret query values: `?key=x&q=1` → `?key=<redacted>&q=1`
/// - webhook URLs: `https://hooks.slack.com/services/<redacted>`
/// - shell assignments: `DB_URL=x cmd`, `export X=y` → `DB_URL=<redacted> cmd`
/// - password arguments of known tools, wherever the tool appears in a segment
///   (`docker exec db mysql -pX`): MySQL/MariaDB `-pX`, `sshpass -p`, `curl`/`wget`
///   `-u`/`--user`, `redis-cli -a`, `docker login -p`, `zip -P`, `7z -pX`,
///   `openssl -pass`/`-passin`/`-passout`/`-k`
///
/// The function is idempotent.
pub fn redact_shell_command(command: &str) -> String {
    let command = command.trim();
    if command.is_empty() {
        return String::new();
    }
    if command.chars().count() > MAX_COMMAND_LENGTH
        || SENSITIVE_COMMAND.is_match(command)
        || should_redact_text(command)
    {
        return REDACTED_COMMAND.to_string();
    }
    let masked = ENV_ASSIGNMENT.replace_all(command, format!("${{1}}${{2}}={REDACTED_COMMAND}").as_str());
    mask_inline_secrets(&masked)
}

/// Mask secrets inside a line and keep the rest: URL credentials, secret URL query
/// values, webhook URLs and password arguments of known tools.
fn mask_inline_secrets(line: &str) -> String {
    let line = URL_USERINFO.replace_all(line, format!("${{1}}{REDACTED_COMMAND}@").as_str());
    let line = URL_QUERY_SECRET.replace_all(&line, format!("${{1}}{REDACTED_COMMAND}").as_str());
    let line = SECRET_URL_PATH.replace_all(&line, format!("${{1}}{REDACTED_COMMAND}").as_str());
    mask_tool_flags(&line)
}

const MYSQL_TOOLS: &[&str] = &[
    "mysql",
    "mysqldump",
    "mysqladmin",
    "mysqlimport",
    "mysqlcheck",
    "mysqlshow",
    "mysqlsh",
    "mysqlpump",
    "mariadb",
    "mariadb-dump",
    "mariadb-admin",
    "mariadb-import",
    "mariadb-check",
];

/// A tool whose password arguments we know how to mask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tool {
    /// `-pSECRET`
    Mysql,
    /// `-p SECRET`, `-pSECRET`
    Sshpass,
    /// `-u user:pw`, `--user=user:pw`, `--http-password pw`
    Curl,
    /// `-a SECRET`, `--pass SECRET`
    Redis,
    /// A container tool. Only its `login` subcommand takes a password.
    Registry,
    /// `docker login -p SECRET`
    Login,
    /// `zip -P SECRET`
    Zip,
    /// `7z -pSECRET`
    SevenZip,
    /// `openssl -pass pass:SECRET`, `-passin`, `-passout`, `enc -k SECRET`
    Openssl,
}

fn tool_named(name: &str) -> Option<Tool> {
    Some(match name {
        _ if MYSQL_TOOLS.contains(&name) => Tool::Mysql,
        "sshpass" => Tool::Sshpass,
        "curl" | "wget" => Tool::Curl,
        "redis-cli" | "valkey-cli" | "keydb-cli" => Tool::Redis,
        "docker" | "podman" | "nerdctl" | "buildah" | "skopeo" | "helm" | "oras" => Tool::Registry,
        "zip" | "unzip" | "zipcloak" => Tool::Zip,
        "7z" | "7za" | "7zr" | "7zz" | "rar" | "unrar" => Tool::SevenZip,
        "openssl" => Tool::Openssl,
        _ => return None,
    })
}

/// URL query parameters whose value is a credential or a signature.
static URL_QUERY_SECRET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)([?&;](?:[a-z0-9.]*[_-])*(?:key|apikey|token|secret|password|passwd|pwd|sig|signature|auth|credential|session|sid|code)(?:[_-][a-z0-9]*)*=)[^&#\s'"]+"#,
    )
    .expect("valid regex")
});
/// URLs whose path is the credential: Slack, Discord and Teams webhooks, Telegram bots.
static SECRET_URL_PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)(hooks\.slack\.com/(?:services|workflows|triggers)/|discord(?:app)?\.com/api/webhooks/|\.webhook\.office\.com/webhookb2/|api\.telegram\.org/bot)[^\s'"]+"#,
    )
    .expect("valid regex")
});

/// Mask tool-specific password arguments, segment by segment. Pipes, lists,
/// subshells, `$(...)` and backticks start a new segment.
fn mask_tool_flags(command: &str) -> String {
    let mut out = String::with_capacity(command.len());
    let mut segment = String::new();
    for ch in command.chars() {
        if matches!(ch, '|' | ';' | '&' | '\n' | '(' | ')' | '`') {
            out.push_str(&mask_segment(&segment));
            out.push(ch);
            segment.clear();
        } else {
            segment.push(ch);
        }
    }
    out.push_str(&mask_segment(&segment));
    out
}

/// The masked form of a value. Closing quotes stay, so `watch 'mysql -pX'` keeps
/// its quoting and a second pass changes nothing.
fn masked(prefix: &str, value: &str) -> Option<String> {
    let bare = value.trim_end_matches(['\'', '"']);
    if bare.is_empty() {
        return None;
    }
    Some(format!("{prefix}{REDACTED_COMMAND}{}", &value[bare.len()..]))
}

/// What to do with one argument of a tool.
enum Mask {
    Keep,
    /// Replace this argument.
    This(String),
    /// The next argument is the secret.
    Next,
}

fn mask_argument(tool: Tool, token: &str) -> Mask {
    let attached = |flag: &str| token.strip_prefix(flag).and_then(|rest| masked(flag, rest));
    let result = match tool {
        Tool::Mysql | Tool::SevenZip => attached("-p"),
        Tool::Sshpass | Tool::Login if token == "-p" => return Mask::Next,
        Tool::Login if token == "--password" => return Mask::Next,
        Tool::Sshpass | Tool::Login => attached("-p"),
        Tool::Curl => {
            if matches!(token, "-u" | "--user" | "--http-password" | "--proxy-user") {
                return Mask::Next;
            }
            match token.split_once('=') {
                Some((flag, _)) if matches!(flag, "--user" | "--http-password" | "--proxy-user") => {
                    Some(format!("{flag}={REDACTED_COMMAND}"))
                }
                _ if !token.starts_with("--") => attached("-u"),
                _ => None,
            }
        }
        Tool::Redis if matches!(token, "-a" | "--pass") => return Mask::Next,
        Tool::Zip if token == "-P" => return Mask::Next,
        Tool::Zip => attached("-P"),
        Tool::Openssl if matches!(token, "-pass" | "-passin" | "-passout" | "-k") => return Mask::Next,
        Tool::Redis | Tool::Registry | Tool::Openssl => None,
    };
    result.map_or(Mask::Keep, Mask::This)
}

fn mask_segment(segment: &str) -> String {
    // Split keeping whitespace runs so the output preserves spacing.
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_space = false;
    for ch in segment.chars() {
        let space = ch.is_whitespace();
        if space != in_space && !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
        in_space = space;
        current.push(ch);
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    // A tool word can come after wrappers (`docker exec db mysql`, `watch`,
    // `kubectl exec ... --`), so every word may switch the rules that apply.
    let mut tool: Option<Tool> = None;
    let mut mask_next = false;
    for token in tokens.iter_mut().filter(|token| !token.trim().is_empty()) {
        if mask_next {
            mask_next = false;
            *token = masked("", token).unwrap_or_else(|| REDACTED_COMMAND.to_string());
            continue;
        }
        let word = token.trim_start_matches(['\'', '"']);
        let name = word.rsplit('/').next().unwrap_or(word).to_ascii_lowercase();
        if let Some(found) = tool_named(&name) {
            tool = Some(found);
            continue;
        }
        if tool == Some(Tool::Registry) && name == "login" {
            tool = Some(Tool::Login);
            continue;
        }
        let Some(current) = tool else { continue };
        match mask_argument(current, token) {
            Mask::Keep => {}
            Mask::This(clean) => *token = clean,
            Mask::Next => mask_next = true,
        }
    }
    tokens.concat()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn raw(source: &str, kind: &str, payload: Value) -> RawEvent {
        RawEvent {
            id: "id".into(),
            ts: 1,
            source: source.into(),
            kind: kind.into(),
            payload: payload.as_object().cloned().unwrap(),
            schema_version: 1,
            ingested_at: None,
        }
    }

    // ---- text detection ---------------------------------------------------

    #[test]
    fn detects_secret_text() {
        for text in [
            "-----BEGIN RSA PRIVATE KEY-----",
            "-----begin openssh private key-----",
            "password = hunter2",
            "API_KEY: x",
            "api-key=abc",
            "accessKey=1",
            "Authorization: Basic x",
            "token=abc",
            "ghp_abcdef123",
            "xoxb-123-abc",
            "eyJhbGciOiJIUzI1NiJ9.payload",
        ] {
            assert!(should_redact_text(text), "{text}");
        }
        for text in ["let x = 1;", "the password field", "tokenize(x)", "max_tokens = 100", "ghp", "eyJ"] {
            assert!(!should_redact_text(text), "{text}");
        }
    }

    #[test]
    fn detects_secret_names_with_prefixes_and_suffixes() {
        for text in [
            "DB_PASSWORD=hunter2",
            "db_password = 1",
            "my GITHUB_TOKEN=x does not work",
            "STRIPE_SECRET_KEY = 'x'",
            "AWS_SECRET_ACCESS_KEY=\"x\"",
            "client-secret: x",
            "\"api_key\": \"x\"",
        ] {
            assert!(should_redact_text(text), "{text}");
        }
    }

    #[test]
    fn detects_more_token_prefixes() {
        for text in [
            "OPENAI = sk-proj-abcdefghijklmnopqrstuvwx",
            "stripe sk_live_abc123def456ghi",
            "stripe sk_test_abc123def456ghi",
            "stripe rk_live_abc123def456ghi",
            concat!("aws AKIA", "IOSFODNN7EXAMPLE"),
            concat!("maps AIza", "SyA-1234567890abcdefghijklmnopqrstu"),
            "gitlab glpat-abcdefghij0123456789",
            "slack xapp-1-A0123-456-abcdef",
        ] {
            assert!(should_redact_text(text), "{text}");
        }
        for text in ["task-list", "pip install sk-learn", "AKIA", "AIza", "glpat"] {
            assert!(!should_redact_text(text), "{text}");
        }
    }

    // ---- document_change --------------------------------------------------

    #[test]
    fn document_change_texts_are_redacted() {
        let event = raw(
            "vscode",
            "document_change",
            json!({"path": "/w/a", "workspace": "/w", "changes": [
                {"kind": "insert", "text": "token = 'abc'", "range": {}, "removed_characters": 0},
                {"kind": "insert", "text": "fine", "range": {}, "removed_characters": 0},
                {"kind": "delete", "range": {}, "removed_characters": 3},
            ]}),
        );
        let out = redact_event(event).unwrap();
        let changes = out.payload["changes"].as_array().unwrap();
        assert_eq!(changes[0]["text"], "[redacted]");
        assert_eq!(changes[0]["redacted"], true);
        assert_eq!(changes[1]["text"], "fine");
        assert!(changes[1].get("redacted").is_none());
        assert!(changes[2].get("text").is_none());
    }

    // ---- firefox ----------------------------------------------------------

    #[test]
    fn firefox_url_is_sanitised() {
        let out = redact_event(raw(
            "firefox",
            "tab_change",
            json!({"url": "https://u:p@Example.com/a?session=1#f", "title": "t"}),
        ))
        .unwrap();
        assert_eq!(out.payload["url"], "https://example.com/a");
        assert_eq!(out.payload["title"], "t");
    }

    #[test]
    fn firefox_non_http_url_is_rejected() {
        for url in ["file:///etc/passwd", "about:config", "moz-extension://x/y", "chrome://settings"] {
            let error = redact_event(raw("firefox", "tab_close", json!({"url": url}))).unwrap_err();
            assert_eq!(error.message, "firefox event url must be an http or https URL");
        }
    }

    #[test]
    fn firefox_target_href_is_sanitised_or_dropped() {
        let out = redact_event(raw(
            "firefox",
            "user_action",
            json!({"url": "https://a.com/", "target": {"tag": "a", "role": "link", "href": "https://b.com/x?q=1"}}),
        ))
        .unwrap();
        assert_eq!(out.payload["target"]["href"], "https://b.com/x");
        let out = redact_event(raw(
            "firefox",
            "user_action",
            json!({"url": "https://a.com/", "target": {"tag": "a", "role": "link", "href": "javascript:void(0)"}}),
        ))
        .unwrap();
        assert!(out.payload["target"].get("href").is_none());
        assert_eq!(out.payload["target"]["tag"], "a");
    }

    #[test]
    fn firefox_secret_context_is_dropped() {
        let event = |excerpt: &str| {
            raw(
                "firefox",
                "user_action",
                json!({"url": "https://a.com/", "context": {"kind": "post", "text_excerpt": excerpt}}),
            )
        };
        assert!(redact_event(event("my password: hunter2")).unwrap().payload.get("context").is_none());
        assert!(redact_event(event("look at ghp_abc123")).unwrap().payload.get("context").is_none());
        assert_eq!(redact_event(event("nice post")).unwrap().payload["context"]["text_excerpt"], "nice post");
    }

    #[test]
    fn firefox_secret_name_in_context_is_dropped() {
        let event = raw(
            "firefox",
            "tab_change",
            json!({"url": "https://a.com/", "context": {"kind": "post", "text_excerpt": "my GITHUB_TOKEN=x does not work"}}),
        );
        assert!(redact_event(event).unwrap().payload.get("context").is_none());
    }

    #[test]
    fn other_sources_untouched() {
        let event = raw("gnome", "something", json!({"app": "x", "title": "password=1"}));
        assert_eq!(redact_event(event.clone()).unwrap(), event);
    }

    // ---- filesystem -------------------------------------------------------

    fn file_content(excerpt: &str) -> RawEvent {
        raw(
            "filesystem",
            "file_content",
            json!({"path": "/w/a", "workspace": "/w", "kind": "text", "mime": "text/plain", "size_bytes": 1, "sha256": "ab", "excerpt": excerpt}),
        )
    }

    #[test]
    fn file_content_secret_excerpt_is_dropped() {
        for excerpt in [
            concat!("-----BEGIN OPENSSH PRIVATE", " KEY-----\nb3BlbnNzaC1rZXk\n"),
            "DEBUG=1\nSECRET_KEY=abc\n",
            "deploy with ghp_abcdef123",
        ] {
            let out = redact_event(file_content(excerpt)).unwrap();
            assert_eq!(out.payload["excerpt"], "[redacted]", "{excerpt}");
            assert_eq!(out.payload["path"], "/w/a");
        }
        let out = redact_event(file_content("# Notes\nplain text")).unwrap();
        assert_eq!(out.payload["excerpt"], "# Notes\nplain text");
    }

    // ---- window titles ----------------------------------------------------

    fn focus(app: &str, title: &str) -> RawEvent {
        raw("linux", "app_focus", json!({"app": app, "title": title, "window_id": "1"}))
    }

    #[test]
    fn private_browsing_titles_are_replaced() {
        for (app, title) in [
            ("firefox", "Chase Online Banking \u{2014} Mozilla Firefox Private Browsing"),
            ("google-chrome", "Chase - Google Chrome (Incognito)"),
            ("chromium", "Chase - Chromium (Incognito)"),
            ("microsoft-edge", "Chase - [InPrivate] - Microsoft Edge"),
            ("brave-browser", "Chase - Brave (Private)"),
        ] {
            let out = redact_event(focus(app, title)).unwrap();
            assert_eq!(out.payload["title"], PRIVATE_WINDOW_TITLE, "{title}");
            assert_eq!(out.payload["app"], app);
            assert_eq!(out.payload["window_id"], "1");
        }
        let out = redact_event(focus("firefox", "Rust docs \u{2014} Mozilla Firefox")).unwrap();
        assert_eq!(out.payload["title"], "Rust docs \u{2014} Mozilla Firefox");
    }

    #[test]
    fn secrets_in_window_titles_are_removed() {
        let out = redact_event(focus("gnome-terminal", "export GITHUB_TOKEN=x")).unwrap();
        assert_eq!(out.payload["title"], "[redacted]");
        let out = redact_event(focus("gnome-terminal", "mysql -uroot -phunter2 app")).unwrap();
        assert_eq!(out.payload["title"], "mysql -uroot -p<redacted> app");
        let out = redact_event(focus("gnome-terminal", "user@host: ~/src")).unwrap();
        assert_eq!(out.payload["title"], "user@host: ~/src");
    }

    // ---- shell ------------------------------------------------------------

    #[test]
    fn shell_event_cmd_is_redacted() {
        let out = redact_event(raw(
            "shell",
            "command",
            json!({"cmd": "export GITHUB_TOKEN=abc", "cwd": "/", "exit_code": 0}),
        ))
        .unwrap();
        assert_eq!(out.payload["cmd"], "<redacted>");
        let out = redact_event(raw("shell", "command", json!({"cmd": "  cargo test  ", "cwd": "/", "exit_code": 0})))
            .unwrap();
        assert_eq!(out.payload["cmd"], "cargo test");
    }

    #[test]
    fn shell_keyword_rules_redact_whole_command() {
        for cmd in [
            "mysql --password=hunter2",
            "echo $PASSWD",
            "aws configure set aws_secret_access_key x",
            "export OPENAI_API_KEY=sk-1",
            "curl -H 'Authorization: Bearer abc' https://x",
            "vault read secret/data",
            "ssh user@host",
            "gpg --decrypt file.gpg",
            "sudo apt install x",
            "git clone https://ghp_abc@github.com/x",
            "echo xoxb-1-2",
            "cat ~/.docker/config.json | grep Credential",
            "kubectl create secret generic x",
            "pssh -h hosts",
        ] {
            assert_eq!(redact_shell_command(cmd), "<redacted>", "{cmd}");
        }
    }

    #[test]
    fn shell_length_limit() {
        let long = format!("echo {}", "a".repeat(496));
        assert_eq!(long.chars().count(), 501);
        assert_eq!(redact_shell_command(&long), "<redacted>");
        let fits = format!("echo {}", "a".repeat(495));
        assert_eq!(redact_shell_command(&fits), fits);
    }

    #[test]
    fn shell_harmless_commands_unchanged() {
        for cmd in [
            "git status",
            "cargo test -p intent-capture",
            "ls -la /tmp",
            "mkdir -p build/out",
            "cp -pr a b",
            "git log --format=%H",
            "python -m pytest -k test_x",
            "npm run build -- --mode=production",
        ] {
            assert_eq!(redact_shell_command(cmd), cmd, "{cmd}");
        }
        assert_eq!(redact_shell_command("   "), "");
    }

    #[test]
    fn shell_env_assignments_masked() {
        assert_eq!(redact_shell_command("DATABASE_URL=postgres://x cargo run"), "DATABASE_URL=<redacted> cargo run");
        assert_eq!(redact_shell_command("A=1 B=\"two words\" make"), "A=<redacted> B=<redacted> make");
        assert_eq!(redact_shell_command("export HF_HOME=/data"), "export HF_HOME=<redacted>");
        assert_eq!(redact_shell_command("env FOO='a b' ./run.sh"), "env FOO=<redacted> ./run.sh");
        assert_eq!(redact_shell_command("make && X=1 ./go"), "make && X=<redacted> ./go");
    }

    #[test]
    fn shell_url_credentials_masked() {
        assert_eq!(
            redact_shell_command("git clone https://alice:hunter2@example.com/repo.git"),
            "git clone https://<redacted>@example.com/repo.git"
        );
        assert_eq!(redact_shell_command("psql postgres://bob:x@db/app"), "psql postgres://<redacted>@db/app");
        assert_eq!(redact_shell_command("curl https://example.com/a@b"), "curl https://example.com/a@b");
    }

    #[test]
    fn shell_password_flags_masked() {
        assert_eq!(redact_shell_command("mysql -uroot -phunter2 app"), "mysql -uroot -p<redacted> app");
        assert_eq!(
            redact_shell_command("/usr/bin/mysqldump -pX db > d.sql"),
            "/usr/bin/mysqldump -p<redacted> db > d.sql"
        );
        assert_eq!(redact_shell_command("cat x.sql | mariadb -pS3cret"), "cat x.sql | mariadb -p<redacted>");
        assert_eq!(redact_shell_command("mysql -p app"), "mysql -p app");
        assert_eq!(redact_shell_command("sshpass -p hunter2 scp a b"), "sshpass -p <redacted> scp a b");
        assert_eq!(redact_shell_command("curl -u alice:pw https://x/"), "curl -u <redacted> https://x/");
        assert_eq!(redact_shell_command("curl --user=alice:pw https://x/"), "curl --user=<redacted> https://x/");
        assert_eq!(redact_shell_command("curl -ualice:pw https://x/"), "curl -u<redacted> https://x/");
        assert_eq!(redact_shell_command("wget --http-password pw https://x/"), "<redacted>");
    }

    #[test]
    fn shell_password_flags_masked_after_wrappers() {
        for (cmd, want) in [
            ("docker exec db mysql -uroot -phunter2", "docker exec db mysql -uroot -p<redacted>"),
            (
                "kubectl exec -it db-0 -- mysql -uroot -phunter2 app",
                "kubectl exec -it db-0 -- mysql -uroot -p<redacted> app",
            ),
            ("watch mysql -phunter2 -e 'show processlist'", "watch mysql -p<redacted> -e 'show processlist'"),
            ("watch 'mysql -phunter2'", "watch 'mysql -p<redacted>'"),
            ("echo $(mysql -phunter2 -e 'select 1')", "echo $(mysql -p<redacted> -e 'select 1')"),
            ("echo `mysql -phunter2`", "echo `mysql -p<redacted>`"),
        ] {
            assert_eq!(redact_shell_command(cmd), want, "{cmd}");
        }
    }

    #[test]
    fn shell_tool_password_arguments_masked() {
        for (cmd, want) in [
            ("redis-cli -a hunter2 ping", "redis-cli -a <redacted> ping"),
            ("redis-cli -h db -a hunter2 --no-auth-warning", "redis-cli -h db -a <redacted> --no-auth-warning"),
            ("docker login -u me -p hunter2 registry.example", "docker login -u me -p <redacted> registry.example"),
            ("podman login -u me -phunter2 quay.io", "podman login -u me -p<redacted> quay.io"),
            ("zip -P hunter2 out.zip a", "zip -P <redacted> out.zip a"),
            ("unzip -P hunter2 a.zip", "unzip -P <redacted> a.zip"),
            ("7z a -phunter2 out.7z dir", "7z a -p<redacted> out.7z dir"),
            ("openssl enc -aes-256-cbc -pass pass:hunter2 -in a", "openssl enc -aes-256-cbc -pass <redacted> -in a"),
            ("openssl pkcs12 -passin pass:a -passout pass:b", "openssl pkcs12 -passin <redacted> -passout <redacted>"),
        ] {
            assert_eq!(redact_shell_command(cmd), want, "{cmd}");
        }
        for cmd in ["tool --password=hunter2", "tool --password hunter2"] {
            assert_eq!(redact_shell_command(cmd), "<redacted>", "{cmd}");
        }
        for cmd in ["docker run -p 8080:80 nginx", "7z x -p a.7z", "zip -r out.zip dir", "redis-cli ping"] {
            assert_eq!(redact_shell_command(cmd), cmd, "{cmd}");
        }
    }

    #[test]
    fn shell_url_query_secrets_masked() {
        for (cmd, want) in [
            ("curl 'https://api.example.com/v1/x?key=abc123'", "curl 'https://api.example.com/v1/x?key=<redacted>'"),
            ("curl https://x.example/a?session_id=abc#top", "curl https://x.example/a?session_id=<redacted>#top"),
            ("curl https://x.example/a?code=abc", "curl https://x.example/a?code=<redacted>"),
            ("wget 'https://b.example/f?sig=abc%3D'", "wget 'https://b.example/f?sig=<redacted>'"),
            ("curl https://x.example/a?X-Amz-Signature=abc", "curl https://x.example/a?X-Amz-Signature=<redacted>"),
            ("curl https://x.example/a?auth=abc#top", "curl https://x.example/a?auth=<redacted>#top"),
        ] {
            assert_eq!(redact_shell_command(cmd), want, "{cmd}");
        }
        assert_eq!(
            redact_shell_command(concat!(
                "curl -s 'https://maps.googleapis.com/maps/api/geocode/json?key=AIza",
                "SyA-1234567890abcdefghijklmnopqrstu'"
            )),
            "<redacted>"
        );
        for cmd in [
            "curl 'https://x.example/a?q=rust'",
            "curl https://x.example/a?keyboard=us",
            "curl https://x.example/a?monkey=1",
        ] {
            assert_eq!(redact_shell_command(cmd), cmd, "{cmd}");
        }
    }

    #[test]
    fn shell_webhook_urls_masked() {
        assert_eq!(
            redact_shell_command("curl -X POST https://hooks.slack.com/services/T0000/B0000/XXXXXXXX -d '{}'"),
            "curl -X POST https://hooks.slack.com/services/<redacted> -d '{}'"
        );
        assert_eq!(
            redact_shell_command("curl https://discord.com/api/webhooks/123/abc-def"),
            "curl https://discord.com/api/webhooks/<redacted>"
        );
    }

    #[test]
    fn shell_redaction_is_idempotent() {
        for cmd in [
            "<redacted>",
            "mysql -uroot -phunter2 app",
            "A=1 make",
            "git clone https://a:b@h/x",
            "sshpass -p pw scp a b",
            "curl -u a:b https://x/",
            "docker exec db mysql -phunter2",
            "watch 'mysql -phunter2'",
            "redis-cli -a pw ping",
            "openssl enc -pass pass:pw",
            "curl 'https://x.example/a?key=abc&q=1'",
            "curl https://hooks.slack.com/services/T0/B0/X",
        ] {
            let once = redact_shell_command(cmd);
            assert_eq!(redact_shell_command(&once), once, "{cmd}");
        }
    }
}
