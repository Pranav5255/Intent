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
/// Commands longer than this (in characters) are fully redacted.
pub const MAX_COMMAND_LENGTH: usize = 500;

static PRIVATE_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----").expect("valid regex")
});
static SENSITIVE_ASSIGNMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:password|passwd|secret|token|api[_-]?key|access[_-]?key|authorization)\b\s*[:=]",
    )
    .expect("valid regex")
});
static TOKEN_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:gh[pous]_[A-Za-z0-9_]+|xox[baprs]-[A-Za-z0-9-]+|eyJ[A-Za-z0-9_-]+\.)")
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
    Regex::new(r#"(^|[\s;&|(])([A-Za-z_][A-Za-z0-9_]*)=("[^"]*"|'[^']*'|[^\s;&|)]*)"#)
        .expect("valid regex")
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
                && let Some(href) = target
                    .get("href")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
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
            if let Some(clean) = payload
                .get("cmd")
                .and_then(Value::as_str)
                .map(redact_shell_command)
            {
                payload.insert("cmd".into(), Value::String(clean));
            }
        }
        _ => {}
    }
    Ok(event)
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
/// - shell assignments: `DB_URL=x cmd`, `export X=y` → `DB_URL=<redacted> cmd`
/// - attached password flags of the MySQL/MariaDB clients: `-pSECRET` → `-p<redacted>`
/// - `sshpass -p PW` / `-pPW`, and `curl`/`wget` `-u user:pw` / `--user=user:pw`
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
    let masked = URL_USERINFO.replace_all(command, format!("${{1}}{REDACTED_COMMAND}@").as_str());
    let masked =
        ENV_ASSIGNMENT.replace_all(&masked, format!("${{1}}${{2}}={REDACTED_COMMAND}").as_str());
    mask_tool_flags(&masked)
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
const WRAPPERS: &[&str] = &[
    "env", "command", "exec", "time", "nice", "nohup", "builtin", "xargs",
];

/// Mask tool-specific password arguments, segment by segment (`|`, `;`, `&`).
fn mask_tool_flags(command: &str) -> String {
    let mut out = String::with_capacity(command.len());
    let mut segment = String::new();
    for ch in command.chars() {
        if matches!(ch, '|' | ';' | '&' | '\n') {
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
    let words: Vec<usize> = (0..tokens.len())
        .filter(|&i| !tokens[i].trim().is_empty())
        .collect();
    let program_index = words.iter().position(|&i| {
        let word = tokens[i].trim_start_matches('(');
        !(word.contains('=') && !word.starts_with('-')) && !WRAPPERS.contains(&word)
    });
    let Some(program_index) = program_index else {
        return segment.to_string();
    };
    let program_word = tokens[words[program_index]].trim_start_matches('(');
    let program = program_word
        .rsplit('/')
        .next()
        .unwrap_or(program_word)
        .to_ascii_lowercase();
    let args = &words[program_index + 1..];
    let mut mask_next = false;
    for &i in args {
        let token = tokens[i].clone();
        if mask_next {
            tokens[i] = REDACTED_COMMAND.to_string();
            mask_next = false;
            continue;
        }
        if MYSQL_TOOLS.contains(&program.as_str()) || program == "sshpass" {
            if let Some(rest) = token.strip_prefix("-p")
                && !rest.is_empty()
            {
                tokens[i] = format!("-p{REDACTED_COMMAND}");
            } else if program == "sshpass" && token == "-p" {
                mask_next = true;
            }
        }
        if program == "curl" || program == "wget" {
            if token == "-u"
                || token == "--user"
                || token == "--http-password"
                || token == "--proxy-user"
            {
                mask_next = true;
            } else if let Some((flag, _)) = token.split_once('=')
                && matches!(flag, "--user" | "--http-password" | "--proxy-user")
            {
                tokens[i] = format!("{flag}={REDACTED_COMMAND}");
            } else if let Some(rest) = token.strip_prefix("-u")
                && !rest.is_empty()
                && !token.starts_with("--")
            {
                tokens[i] = format!("-u{REDACTED_COMMAND}");
            }
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
        for text in [
            "let x = 1;",
            "the password field",
            "tokenize(x)",
            "db_password = 1",
            "ghp",
            "eyJ",
        ] {
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
        for url in [
            "file:///etc/passwd",
            "about:config",
            "moz-extension://x/y",
            "chrome://settings",
        ] {
            let error = redact_event(raw("firefox", "tab_close", json!({"url": url}))).unwrap_err();
            assert_eq!(
                error.message,
                "firefox event url must be an http or https URL"
            );
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
        assert!(
            redact_event(event("my password: hunter2"))
                .unwrap()
                .payload
                .get("context")
                .is_none()
        );
        assert!(
            redact_event(event("look at ghp_abc123"))
                .unwrap()
                .payload
                .get("context")
                .is_none()
        );
        assert_eq!(
            redact_event(event("nice post")).unwrap().payload["context"]["text_excerpt"],
            "nice post"
        );
    }

    #[test]
    fn other_sources_untouched() {
        let event = raw(
            "linux",
            "app_focus",
            json!({"app": "x", "title": "password=1"}),
        );
        assert_eq!(redact_event(event.clone()).unwrap(), event);
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
        let out = redact_event(raw(
            "shell",
            "command",
            json!({"cmd": "  cargo test  ", "cwd": "/", "exit_code": 0}),
        ))
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
        assert_eq!(
            redact_shell_command("DATABASE_URL=postgres://x cargo run"),
            "DATABASE_URL=<redacted> cargo run"
        );
        assert_eq!(
            redact_shell_command("A=1 B=\"two words\" make"),
            "A=<redacted> B=<redacted> make"
        );
        assert_eq!(
            redact_shell_command("export HF_HOME=/data"),
            "export HF_HOME=<redacted>"
        );
        assert_eq!(
            redact_shell_command("env FOO='a b' ./run.sh"),
            "env FOO=<redacted> ./run.sh"
        );
        assert_eq!(
            redact_shell_command("make && X=1 ./go"),
            "make && X=<redacted> ./go"
        );
    }

    #[test]
    fn shell_url_credentials_masked() {
        assert_eq!(
            redact_shell_command("git clone https://alice:hunter2@example.com/repo.git"),
            "git clone https://<redacted>@example.com/repo.git"
        );
        assert_eq!(
            redact_shell_command("psql postgres://bob:x@db/app"),
            "psql postgres://<redacted>@db/app"
        );
        assert_eq!(
            redact_shell_command("curl https://example.com/a@b"),
            "curl https://example.com/a@b"
        );
    }

    #[test]
    fn shell_password_flags_masked() {
        assert_eq!(
            redact_shell_command("mysql -uroot -phunter2 app"),
            "mysql -uroot -p<redacted> app"
        );
        assert_eq!(
            redact_shell_command("/usr/bin/mysqldump -pX db > d.sql"),
            "/usr/bin/mysqldump -p<redacted> db > d.sql"
        );
        assert_eq!(
            redact_shell_command("cat x.sql | mariadb -pS3cret"),
            "cat x.sql | mariadb -p<redacted>"
        );
        assert_eq!(redact_shell_command("mysql -p app"), "mysql -p app");
        assert_eq!(
            redact_shell_command("sshpass -p hunter2 scp a b"),
            "sshpass -p <redacted> scp a b"
        );
        assert_eq!(
            redact_shell_command("curl -u alice:pw https://x/"),
            "curl -u <redacted> https://x/"
        );
        assert_eq!(
            redact_shell_command("curl --user=alice:pw https://x/"),
            "curl --user=<redacted> https://x/"
        );
        assert_eq!(
            redact_shell_command("curl -ualice:pw https://x/"),
            "curl -u<redacted> https://x/"
        );
        assert_eq!(
            redact_shell_command("wget --http-password pw https://x/"),
            "<redacted>"
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
        ] {
            let once = redact_shell_command(cmd);
            assert_eq!(redact_shell_command(&once), once, "{cmd}");
        }
    }
}
