//! `intentctl` — command-line control for `intentd`.
//!
//! `send-shell` is on the hot path of every interactive shell command, so it does no
//! async setup, uses short timeouts and never reports failures to the terminal.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::{Value, json};

const SHELL_BLOCK_START: &str = "# >>> intent shell hook >>>";
const SHELL_BLOCK_END: &str = "# <<< intent shell hook <<<";
const SERVICE: &str = "intentd.service";
const GNOME_EXTENSION_UUID: &str = "intent-focus@intent.local";

#[derive(Parser)]
#[command(name = "intentctl", version, about = "Control the Intent daemon")]
struct Cli {
    /// Daemon base URL.
    #[arg(long, env = "INTENT_URL", default_value = "http://127.0.0.1:9477", global = true)]
    url: String,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Report one finished shell command (used by the shell hooks).
    SendShell {
        #[arg(long)]
        cmd: String,
        #[arg(long)]
        cwd: String,
        #[arg(long = "exit", allow_negative_numbers = true)]
        exit_code: i64,
        #[arg(long)]
        duration_ms: Option<u64>,
    },
    /// Show daemon, collector and service health.
    Status,
    /// Install and start the user service, shell hooks and companions.
    Enable {
        /// Skip installing shell hooks.
        #[arg(long)]
        no_shell: bool,
    },
    /// Stop and disable the user service.
    Disable,
    /// Pause capture, optionally for a duration such as 15m, 1h or 2h30m.
    Pause { duration: Option<String> },
    /// Resume capture.
    Resume,
    /// Manage the shell hooks.
    Shell {
        action: Toggle,
        #[arg(long, value_enum)]
        shell: Option<ShellKind>,
    },
    /// Manage workspaces approved for detailed editor and filesystem capture.
    Workspace {
        #[command(subcommand)]
        action: WorkspaceCmd,
    },
    /// Run the intent pipeline now.
    Run {
        /// Day to process (YYYY-MM-DD); defaults to today.
        #[arg(long)]
        date: Option<String>,
    },
    /// Print one day's raw events as JSON.
    ExportDay {
        #[arg(long)]
        date: String,
    },
    /// Remove services, hooks and (unless --keep-data) all captured data.
    Uninstall {
        #[arg(long)]
        keep_data: bool,
    },
}

#[derive(Subcommand)]
enum WorkspaceCmd {
    Add { path: PathBuf },
    Remove { path: PathBuf },
    List,
}

#[derive(Clone, Copy, ValueEnum)]
enum Toggle {
    Enable,
    Disable,
}

#[derive(Clone, Copy, ValueEnum)]
enum ShellKind {
    Bash,
    Zsh,
}

impl ShellKind {
    fn name(self) -> &'static str {
        match self {
            ShellKind::Bash => "bash",
            ShellKind::Zsh => "zsh",
        }
    }

    fn rc_file(self) -> PathBuf {
        home().join(match self {
            ShellKind::Bash => ".bashrc",
            ShellKind::Zsh => ".zshrc",
        })
    }

    fn detect() -> ShellKind {
        match std::env::var("SHELL").unwrap_or_default().rsplit('/').next() {
            Some("zsh") => ShellKind::Zsh,
            _ => ShellKind::Bash,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Cmd::SendShell { cmd, cwd, exit_code, duration_ms } = &cli.command {
        // Never surface errors into the user's prompt.
        let _ = send_shell(&cli.url, cmd, cwd, *exit_code, *duration_ms);
        return ExitCode::SUCCESS;
    }
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("intentctl: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let api = Api::new(&cli.url);
    match cli.command {
        Cmd::SendShell { .. } => unreachable!("handled in main"),
        Cmd::Status => status(&api),
        Cmd::Enable { no_shell } => enable(no_shell),
        Cmd::Disable => systemctl(&["disable", "--now", SERVICE]),
        Cmd::Pause { duration } => {
            let seconds = duration.as_deref().map(parse_duration).transpose()?;
            let res =
                api.send("POST", "/v1/capture/pause", Some(json!({ "paused": true, "duration_seconds": seconds })))?;
            print_json(&res);
            Ok(())
        }
        Cmd::Resume => {
            print_json(&api.send("POST", "/v1/capture/pause", Some(json!({ "paused": false })))?);
            Ok(())
        }
        Cmd::Shell { action, shell } => {
            let shell = shell.unwrap_or_else(ShellKind::detect);
            match action {
                Toggle::Enable => install_shell_hook(shell),
                Toggle::Disable => remove_shell_hook(shell),
            }
        }
        Cmd::Workspace { action } => workspace(&api, action),
        Cmd::Run { date } => {
            let path = match date {
                Some(d) => format!("/pipeline/run?date={d}"),
                None => "/pipeline/run".to_string(),
            };
            // A local model can take minutes per day on CPU; don't give up at the default 30 s.
            let res = Api { agent: agent(Duration::from_secs(900)), ..api }.send("POST", &path, None)?;
            let count = res["intents"].as_array().map_or(0, Vec::len);
            println!("{count} intents (cached: {})", res["cached"]);
            Ok(())
        }
        Cmd::ExportDay { date } => {
            print_json(&api.send("GET", &format!("/v1/export/day?date={date}"), None)?);
            Ok(())
        }
        Cmd::Uninstall { keep_data } => uninstall(keep_data),
    }
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

struct Api {
    base: String,
    agent: ureq::Agent,
}

impl Api {
    fn new(base: &str) -> Self {
        Self { base: base.trim_end_matches('/').to_string(), agent: agent(Duration::from_secs(30)) }
    }

    fn send(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value> {
        let url = format!("{}{path}", self.base);
        let token = read_token();
        let mut response = match (method, body) {
            ("GET", _) => with_auth(self.agent.get(&url), token.as_deref()).call(),
            ("DELETE", _) => with_auth(self.agent.delete(&url), token.as_deref()).call(),
            (m, body) => {
                let req = match m {
                    "PUT" => self.agent.put(&url),
                    _ => self.agent.post(&url),
                };
                with_auth(req, token.as_deref()).send_json(body.unwrap_or(Value::Null))
            }
        }
        .with_context(|| format!("{method} {path} failed (is intentd running? try `intentctl status`)"))?;
        let status = response.status();
        if status == 204 {
            return Ok(Value::Null);
        }
        if status.is_client_error() || status.is_server_error() {
            // The daemon explains the refusal in `detail`; show that, not just the code.
            let body = response.body_mut().read_json::<Value>().unwrap_or(Value::Null);
            match body["detail"].as_str() {
                Some(detail) => bail!("{detail}"),
                None => bail!("{method} {path} failed with HTTP {}", status.as_u16()),
            }
        }
        Ok(response.body_mut().read_json::<Value>()?)
    }
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(timeout)).http_status_as_error(false).build().into()
}

fn with_auth<B>(req: ureq::RequestBuilder<B>, token: Option<&str>) -> ureq::RequestBuilder<B> {
    match token {
        Some(t) => req.header("Authorization", &format!("Bearer {t}")),
        None => req,
    }
}

fn token_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") {
        paths.push(PathBuf::from(runtime).join("intent/token"));
    }
    paths.push(data_dir().join("token"));
    paths
}

fn read_token() -> Option<String> {
    token_paths().iter().find_map(|p| fs::read_to_string(p).ok()).map(|t| t.trim().to_string())
}

/// The server fully redacts a command longer than this many characters.
const SERVER_MAX_COMMAND_CHARS: usize = 500;

/// The hooks treat `INTENT_PAUSED` unset, empty or `0` as not paused. Match them.
fn shell_paused(value: Option<&OsStr>) -> bool {
    value.is_some_and(|v| !v.is_empty() && v != "0")
}

/// The server owns the long-command rule. If we cut a long command to the limit,
/// the server would store the start of it. One character over the limit is enough
/// for the server to see that it is too long, and it keeps the request small.
fn shell_cmd_to_send(cmd: &str) -> String {
    truncate(cmd, SERVER_MAX_COMMAND_CHARS + 1)
}

fn send_shell(base: &str, cmd: &str, cwd: &str, exit_code: i64, duration_ms: Option<u64>) -> Result<()> {
    if shell_paused(std::env::var_os("INTENT_PAUSED").as_deref()) || cmd.trim().is_empty() {
        return Ok(());
    }
    let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let mut payload = json!({
        "cmd": shell_cmd_to_send(cmd),
        "cwd": truncate(cwd, 4096),
        "exit_code": exit_code,
    });
    if let Some(ms) = duration_ms {
        payload["duration_ms"] = json!(ms);
    }
    let event = json!({
        "id": uuid::Uuid::new_v4().to_string(),
        "schema_version": 1,
        "ts": ts,
        "source": "shell",
        "type": "command",
        "payload": payload,
    });
    // Redaction happens server-side; this sender stays dumb and fast.
    agent(Duration::from_millis(300)).post(&format!("{}/v1/event", base.trim_end_matches('/'))).send_json(event)?;
    Ok(())
}

fn truncate(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

fn print_json(value: &Value) {
    println!("{}", serde_json::to_string_pretty(value).unwrap_or_default());
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn status(api: &Api) -> Result<()> {
    let service = Command::new("systemctl")
        .args(["--user", "is-active", SERVICE])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "unknown".into());
    let daemon = api.send("GET", "/v1/status", None);
    print_json(&json!({
        "service": service,
        "daemon": daemon.as_ref().map_or_else(|e| json!({ "error": e.to_string() }), Clone::clone),
    }));
    Ok(())
}

fn enable(no_shell: bool) -> Result<()> {
    let _ = systemctl(&["import-environment", "DISPLAY", "XAUTHORITY", "WAYLAND_DISPLAY", "XDG_SESSION_TYPE"]);
    systemctl(&["daemon-reload"])?;
    systemctl(&["enable", "--now", SERVICE])?;
    if !no_shell {
        install_shell_hook(ShellKind::detect())?;
    }
    if which("gnome-extensions") {
        // Harmless if the extension isn't installed or the session isn't GNOME.
        let _ = Command::new("gnome-extensions").args(["enable", GNOME_EXTENSION_UUID]).status();
    }
    install_vscode_extension();
    println!("Intent is enabled. Open a new terminal for shell capture.");
    Ok(())
}

fn uninstall(keep_data: bool) -> Result<()> {
    let _ = systemctl(&["disable", "--now", SERVICE]);
    for shell in [ShellKind::Bash, ShellKind::Zsh] {
        remove_shell_hook(shell)?;
    }
    for editor in ["code", "codium", "cursor"] {
        if which(editor) {
            let _ = Command::new(editor).args(["--uninstall-extension", "intent.intent-vscode"]).status();
        }
    }
    if !keep_data {
        for dir in [config_dir(), data_dir()] {
            if dir.exists() {
                fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
            }
        }
    }
    println!(
        "Intent services and hooks removed{}.",
        if keep_data { "; data kept" } else { " along with captured data" }
    );
    Ok(())
}

fn workspace(api: &Api, action: WorkspaceCmd) -> Result<()> {
    let current = api.send("GET", "/v1/workspaces", None)?;
    let mut list: Vec<String> = serde_json::from_value(current["workspaces"].clone()).unwrap_or_default();
    match action {
        WorkspaceCmd::List => {
            list.iter().for_each(|w| println!("{w}"));
            return Ok(());
        }
        WorkspaceCmd::Add { path } => {
            let path = fs::canonicalize(&path).with_context(|| format!("{} does not exist", path.display()))?;
            check_addable(&path, &home())?;
            let path = path.to_string_lossy().into_owned();
            if !list.contains(&path) {
                list.push(path);
            }
        }
        WorkspaceCmd::Remove { path } => {
            let cwd = std::env::current_dir().context("reading the current directory")?;
            remove_entry(&mut list, &path, &cwd)?;
        }
    }
    // The daemon re-applies the watcher configuration on write; no restart needed.
    api.send("PUT", "/v1/workspaces", Some(json!({ "workspaces": list })))?;
    list.iter().for_each(|w| println!("{w}"));
    Ok(())
}

/// Absolute form of `path` without requiring it to exist: relative to `cwd`, `.`
/// and `..` removed, and symlinks resolved for the part that exists (as the daemon
/// stores workspaces).
fn resolve_path(path: &Path, cwd: &Path) -> PathBuf {
    let mut lexical = PathBuf::from("/");
    for component in cwd.join(path).components() {
        match component {
            std::path::Component::Normal(part) => lexical.push(part),
            std::path::Component::ParentDir => {
                lexical.pop();
            }
            _ => {}
        }
    }
    for base in lexical.ancestors() {
        if let Ok(real) = fs::canonicalize(base) {
            let rest = lexical.strip_prefix(base).unwrap_or(Path::new(""));
            return if rest.as_os_str().is_empty() { real } else { real.join(rest) };
        }
    }
    lexical
}

/// Refuse what the daemon refuses, with the same reason, before asking it.
fn check_addable(path: &Path, home: &Path) -> Result<()> {
    if path.parent().is_none() {
        bail!("/ cannot be a workspace because it holds every file on the computer");
    }
    let home = fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    if path == home {
        bail!("{} is your home directory and cannot be a workspace; add a project folder inside it", path.display());
    }
    Ok(())
}

/// Remove the entry that `path` names, whether or not the folder still exists.
fn remove_entry(list: &mut Vec<String>, path: &Path, cwd: &Path) -> Result<String> {
    let target = resolve_path(path, cwd).to_string_lossy().into_owned();
    let before = list.len();
    list.retain(|w| *w != target);
    if list.len() == before {
        bail!("{target} is not an approved workspace (see `intentctl workspace list`)");
    }
    Ok(target)
}

fn install_shell_hook(shell: ShellKind) -> Result<()> {
    let hook = find_shell_hook(shell).context("shell hook script not found; is Intent installed?")?;
    let rc = shell.rc_file();
    let existing = fs::read_to_string(&rc).unwrap_or_default();
    let cleaned = strip_block(&existing);
    let block = format!("{SHELL_BLOCK_START}\n[ -r \"{0}\" ] && . \"{0}\"\n{SHELL_BLOCK_END}\n", hook.display());
    let mut next = cleaned.trim_end().to_string();
    if !next.is_empty() {
        next.push_str("\n\n");
    }
    next.push_str(&block);
    fs::write(&rc, next).with_context(|| format!("writing {}", rc.display()))?;
    println!("Installed {} hook in {}", shell.name(), rc.display());
    Ok(())
}

fn remove_shell_hook(shell: ShellKind) -> Result<()> {
    let rc = shell.rc_file();
    let Ok(existing) = fs::read_to_string(&rc) else { return Ok(()) };
    let cleaned = strip_block(&existing);
    if cleaned != existing {
        fs::write(&rc, cleaned)?;
        println!("Removed {} hook from {}", shell.name(), rc.display());
    }
    Ok(())
}

/// Removes the marked hook block, leaving the rest of the rc file byte-identical.
fn strip_block(text: &str) -> String {
    let Some(start) = text.find(SHELL_BLOCK_START) else { return text.to_string() };
    let Some(end_rel) = text[start..].find(SHELL_BLOCK_END) else { return text.to_string() };
    let mut end = start + end_rel + SHELL_BLOCK_END.len();
    if text[end..].starts_with('\n') {
        end += 1;
    }
    format!("{}{}", &text[..start], &text[end..])
}

fn find_shell_hook(shell: ShellKind) -> Option<PathBuf> {
    let file = format!("intent.{}", shell.name());
    let mut candidates = vec![PathBuf::from("/usr/share/intent/shell").join(&file)];
    if let Ok(exe) = std::env::current_exe() {
        // Development checkout: target/<profile>/intentctl → <repo>/shell/.
        if let Some(repo) = exe.ancestors().nth(3) {
            candidates.push(repo.join("shell").join(&file));
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn install_vscode_extension() {
    let vsix = Path::new("/usr/share/intent/integrations/intent-vscode.vsix");
    if !vsix.is_file() {
        return;
    }
    for editor in ["code", "codium", "cursor"] {
        if which(editor) {
            let _ = Command::new(editor).arg("--install-extension").arg(vsix).arg("--force").status();
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn systemctl(args: &[&str]) -> Result<()> {
    let status = Command::new("systemctl").arg("--user").args(args).status().context("running systemctl")?;
    if !status.success() {
        bail!("systemctl --user {} failed", args.join(" "));
    }
    Ok(())
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(bin).is_file()))
        .unwrap_or(false)
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg_dir(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(fallback))
        .join("intent")
}

fn config_dir() -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", ".config")
}

fn data_dir() -> PathBuf {
    xdg_dir("XDG_DATA_HOME", ".local/share")
}

/// Parses durations like `15m`, `1h`, `2h30m`, `90s` or a bare number of minutes.
fn parse_duration(text: &str) -> Result<u64> {
    let text = text.trim();
    if let Ok(minutes) = text.parse::<u64>() {
        return Ok(minutes * 60);
    }
    let mut total = 0;
    let mut number = String::new();
    for c in text.chars() {
        if c.is_ascii_digit() {
            number.push(c);
            continue;
        }
        let n: u64 = number.parse().with_context(|| format!("invalid duration {text:?}"))?;
        total += n * match c {
            'h' => 3600,
            'm' => 60,
            's' => 1,
            _ => bail!("invalid duration unit {c:?} in {text:?}"),
        };
        number.clear();
    }
    if !number.is_empty() {
        bail!("duration {text:?} is missing a unit");
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("15m").unwrap(), 900);
        assert_eq!(parse_duration("1h").unwrap(), 3600);
        assert_eq!(parse_duration("2h30m").unwrap(), 9000);
        assert_eq!(parse_duration("45").unwrap(), 2700);
        assert!(parse_duration("10x").is_err());
        assert!(parse_duration("1h30").is_err());
    }

    #[test]
    fn strip_block_preserves_surroundings() {
        let rc = format!("alias ll='ls -l'\n{SHELL_BLOCK_START}\n. /x\n{SHELL_BLOCK_END}\nexport A=1\n");
        assert_eq!(strip_block(&rc), "alias ll='ls -l'\nexport A=1\n");
        assert_eq!(strip_block("plain\n"), "plain\n");
    }

    #[test]
    fn removes_a_workspace_that_no_longer_exists() {
        let cwd = Path::new("/nonexistent-intent/home");
        let mut list = vec!["/nonexistent-intent/home/tmpws".to_string(), "/nonexistent-intent/home/src".to_string()];
        let removed = remove_entry(&mut list, Path::new("tmpws"), cwd).unwrap();
        assert_eq!(removed, "/nonexistent-intent/home/tmpws");
        assert_eq!(list, ["/nonexistent-intent/home/src"]);
        remove_entry(&mut list, Path::new("../home/./src/"), cwd).unwrap();
        assert!(list.is_empty());
        let err = remove_entry(&mut list, Path::new("nope"), cwd).unwrap_err().to_string();
        assert!(err.contains("/nonexistent-intent/home/nope"), "{err}");
    }

    #[test]
    fn refuses_root_and_home() {
        let home = Path::new("/nonexistent-intent/home");
        assert!(check_addable(Path::new("/"), home).is_err());
        assert!(check_addable(home, home).unwrap_err().to_string().contains("home directory"));
        assert!(check_addable(&home.join("src"), home).is_ok());
    }

    #[test]
    fn removes_a_workspace_through_a_symlink() {
        let dir = std::env::temp_dir().join(format!("intentctl-test-{}", std::process::id()));
        let real = dir.join("real");
        fs::create_dir_all(&real).unwrap();
        let real = fs::canonicalize(&real).unwrap();
        #[cfg(unix)]
        {
            let link = dir.join("link");
            let _ = fs::remove_file(&link);
            std::os::unix::fs::symlink(&real, &link).unwrap();
            let mut list = vec![real.to_string_lossy().into_owned()];
            remove_entry(&mut list, &link, Path::new("/")).unwrap();
            assert!(list.is_empty());
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn paused_only_for_a_value_other_than_zero() {
        assert!(!shell_paused(None));
        assert!(!shell_paused(Some(OsStr::new(""))));
        assert!(!shell_paused(Some(OsStr::new("0"))));
        assert!(shell_paused(Some(OsStr::new("1"))));
        assert!(shell_paused(Some(OsStr::new("yes"))));
    }

    #[test]
    fn long_command_stays_long_enough_to_be_redacted() {
        let long = "x".repeat(2000);
        assert_eq!(shell_cmd_to_send(&long).chars().count(), 501);
        assert_eq!(shell_cmd_to_send("ls -la"), "ls -la");
    }

    #[test]
    fn truncate_is_char_safe() {
        assert_eq!(truncate("héllo", 2), "hé");
    }
}
