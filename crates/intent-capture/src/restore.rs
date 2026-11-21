//! Restore a resumed intent: open files, URLs and a terminal,
//! after validating every target.
//!
//! Split in two so the decision logic is testable:
//! - [`plan_restore`] / [`plan_restore_with`] — decide which programs to launch.
//! - [`execute`] — launch them (no shell, new session, stdio detached).
//!
//! 2.0 changes: launchers are detected instead of hard-coding VS Code / Firefox /
//! GNOME Terminal; `last_cmd` is never executed or pre-filled.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use intent_types::{RESUME_MAX_FILES, RESUME_MAX_URLS, ResumePayload, ShellResume};
use serde::{Deserialize, Serialize};

use crate::paths::{expand_user, resolve_lenient};
use crate::url::urlsplit;
use crate::validate::ValidationError;

/// Maximum `shell.last_cmd` length accepted on the wire.
pub const MAX_LAST_CMD_CHARS: usize = 500;

/// Editors tried in order before `$VISUAL`.
pub const EDITORS: &[&str] = &["code", "codium", "cursor"];
/// Terminal emulators tried in order.
pub const TERMINALS: &[&str] = &["gnome-terminal", "ptyxis", "kgx", "konsole", "xfce4-terminal", "x-terminal-emulator"];
/// macOS editor apps tried in order when no editor command is on `$PATH`.
pub const MAC_EDITORS: &[&str] = &["Visual Studio Code", "Cursor", "VSCodium"];
/// macOS terminal apps tried in order. Terminal ships with every Mac.
pub const MAC_TERMINALS: &[&str] = &["iTerm", "Terminal"];

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RestoreMode {
    /// Fresh windows.
    #[default]
    Resume,
    /// Add to the current session: skip files already open, open terminal tabs.
    Continue,
}

/// `POST /v1/restore` body (1.x `ResumePayload` with `mode`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RestoreRequest {
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub urls: Vec<String>,
    #[serde(default)]
    pub shell: ShellResume,
}

fn default_mode() -> String {
    "resume".into()
}

impl RestoreRequest {
    /// 1.x validation: de-duplicate, at most 5 files / 8 URLs, URLs must be
    /// http(s) with a host, `last_cmd` at most 500 characters.
    pub fn validate(self) -> Result<(ResumePayload, RestoreMode), ValidationError> {
        let mode = match self.mode.as_str() {
            "resume" => RestoreMode::Resume,
            "continue" => RestoreMode::Continue,
            _ => return Err(ValidationError::new("mode must be resume or continue")),
        };
        let files = dedupe(self.files);
        if files.len() > RESUME_MAX_FILES {
            return Err(ValidationError::new("at most five files may be restored"));
        }
        let urls = dedupe(self.urls);
        if urls.len() > RESUME_MAX_URLS {
            return Err(ValidationError::new("at most eight URLs may be restored"));
        }
        if !urls.iter().all(|url| is_http_url(url)) {
            return Err(ValidationError::new("urls must use http or https"));
        }
        if self.shell.last_cmd.as_ref().is_some_and(|cmd| cmd.chars().count() > MAX_LAST_CMD_CHARS) {
            return Err(ValidationError::new("shell.last_cmd must be at most 500 characters"));
        }
        Ok((ResumePayload { files, urls, shell: self.shell }, mode))
    }
}

fn dedupe(items: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(items.len());
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

fn is_http_url(url: &str) -> bool {
    urlsplit(url).is_ok_and(|parsed| (parsed.scheme == "http" || parsed.scheme == "https") && !parsed.netloc.is_empty())
}

/// What a launch restores when it succeeds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoredCounts {
    pub files: usize,
    pub urls: usize,
    pub shell: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchTarget {
    Editor,
    Browser,
    Terminal,
}

impl LaunchTarget {
    fn label(self) -> &'static str {
        match self {
            LaunchTarget::Editor => "editor",
            LaunchTarget::Browser => "browser",
            LaunchTarget::Terminal => "terminal",
        }
    }
}

/// One program invocation (argv only — never a shell string).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchCommand {
    pub target: LaunchTarget,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub restores: RestoredCounts,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestorePlan {
    pub mode: RestoreMode,
    pub commands: Vec<LaunchCommand>,
    /// Problems found while planning (missing files, no launcher, …).
    pub failed: Vec<String>,
    /// Files skipped in continue mode because they already appear open.
    pub already_open: Vec<String>,
}

/// `POST /v1/restore` response (1.x shape).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreResult {
    pub ok: bool,
    pub restored: RestoredCounts,
    pub failed: Vec<String>,
}

/// Host facts the planner depends on; injectable for tests.
pub trait SystemProbe {
    /// Absolute path of an executable on `$PATH`.
    fn which(&self, program: &str) -> Option<PathBuf>;
    fn is_file(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;
    fn env(&self, key: &str) -> Option<String>;
    /// Titles of open top-level windows (`wmctrl -l`), when they can be listed.
    fn window_titles(&self) -> Option<String>;
    /// Whether this is macOS, where apps open through `open -a`.
    fn is_macos(&self) -> bool {
        false
    }
    /// Whether a macOS app bundle (`/Applications/<name>.app`) is installed.
    fn has_mac_app(&self, _name: &str) -> bool {
        false
    }
}

/// The real host.
#[derive(Debug, Clone, Copy, Default)]
pub struct HostProbe;

impl SystemProbe for HostProbe {
    fn which(&self, program: &str) -> Option<PathBuf> {
        find_executable(program, std::env::var_os("PATH").as_deref())
    }

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn env(&self, key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|value| !value.trim().is_empty())
    }

    fn window_titles(&self) -> Option<String> {
        // wmctrl only sees X11 clients; on Wayland it would miss native windows, so
        // skip the dedupe there instead of trusting a partial list.
        if !is_x11_session(self) {
            return None;
        }
        let wmctrl = self.which("wmctrl")?;
        run_with_timeout(Command::new(wmctrl).arg("-l"), Duration::from_secs(1))
    }

    fn is_macos(&self) -> bool {
        cfg!(target_os = "macos")
    }

    fn has_mac_app(&self, name: &str) -> bool {
        let bundle = format!("{name}.app");
        let system = Path::new("/Applications").join(&bundle);
        let user = std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Applications").join(&bundle));
        system.is_dir() || user.is_some_and(|path| path.is_dir())
    }
}

/// `open -a <app> <args…>` on macOS, when `open` and the app exist.
fn mac_open(probe: &dyn SystemProbe, app: Option<&str>, rest: Vec<String>) -> Option<(PathBuf, Vec<String>)> {
    if !probe.is_macos() {
        return None;
    }
    let open = probe.which("open")?;
    let mut args = Vec::new();
    if let Some(app) = app {
        if !probe.has_mac_app(app) {
            return None;
        }
        args.extend(["-a".to_string(), app.to_string()]);
    }
    args.extend(rest);
    Some((open, args))
}

fn is_x11_session(probe: &dyn SystemProbe) -> bool {
    match probe.env("XDG_SESSION_TYPE").as_deref() {
        Some("x11") => true,
        Some("wayland") => false,
        _ => probe.env("DISPLAY").is_some() && probe.env("WAYLAND_DISPLAY").is_none(),
    }
}

fn find_executable(program: &str, path_var: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    let candidate = Path::new(program);
    if candidate.components().count() > 1 {
        return is_executable(candidate).then(|| candidate.to_path_buf());
    }
    std::env::split_paths(path_var?)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program))
        .find(|path| is_executable(path))
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

fn run_with_timeout(command: &mut Command, timeout: Duration) -> Option<String> {
    use std::io::Read;
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() < timeout => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut output = String::new();
    child.stdout.take()?.read_to_string(&mut output).ok()?;
    Some(output)
}

/// Plan against the real host.
pub fn plan_restore(payload: &ResumePayload, mode: RestoreMode) -> RestorePlan {
    plan_restore_with(payload, mode, &HostProbe)
}

/// Plan against an arbitrary [`SystemProbe`].
pub fn plan_restore_with(payload: &ResumePayload, mode: RestoreMode, probe: &dyn SystemProbe) -> RestorePlan {
    let mut plan = RestorePlan { mode, ..Default::default() };
    plan_files(payload, mode, probe, &mut plan);
    plan_urls(payload, probe, &mut plan);
    plan_terminal(&payload.shell, mode, probe, &mut plan);
    plan
}

fn plan_files(payload: &ResumePayload, mode: RestoreMode, probe: &dyn SystemProbe, plan: &mut RestorePlan) {
    let mut existing: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for raw in dedupe(payload.files.clone()).into_iter().take(RESUME_MAX_FILES) {
        let expanded = expand_user(Path::new(&raw));
        if probe.is_file(&expanded) {
            let resolved = resolve_lenient(&expanded).to_string_lossy().into_owned();
            if !existing.contains(&resolved) {
                existing.push(resolved);
            }
        } else {
            missing.push(expanded.to_string_lossy().into_owned());
        }
    }
    if !missing.is_empty() {
        plan.failed.push(format!("files not found: {}", missing.join(", ")));
    }
    if mode == RestoreMode::Continue
        && let Some(titles) = probe.window_titles()
    {
        // Window titles, not editor tabs: errs on the side of reopening.
        let (open, closed): (Vec<String>, Vec<String>) = existing.into_iter().partition(|file| {
            Path::new(file).file_name().is_some_and(|name| titles.contains(name.to_string_lossy().as_ref()))
        });
        plan.already_open = open;
        existing = closed;
    }
    if existing.is_empty() {
        return;
    }
    let editor = EDITORS
        .iter()
        .find_map(|name| probe.which(name).map(|path| (path, vec!["--reuse-window".to_string()])))
        .or_else(|| {
            let visual = probe.env("VISUAL")?;
            let mut words = visual.split_whitespace();
            let program = probe.which(words.next()?)?;
            Some((program, words.map(str::to_owned).collect()))
        })
        .or_else(|| MAC_EDITORS.iter().find_map(|app| mac_open(probe, Some(app), Vec::new())));
    match editor {
        Some((program, mut args)) => {
            let count = existing.len();
            args.extend(existing);
            plan.commands.push(LaunchCommand {
                target: LaunchTarget::Editor,
                program,
                args,
                cwd: None,
                restores: RestoredCounts { files: count, ..Default::default() },
            });
        }
        None => plan.failed.push("no editor found (tried code, codium, cursor, $VISUAL)".into()),
    }
}

fn plan_urls(payload: &ResumePayload, probe: &dyn SystemProbe, plan: &mut RestorePlan) {
    let all = dedupe(payload.urls.clone());
    let (urls, rejected): (Vec<String>, Vec<String>) = all.into_iter().partition(|url| is_http_url(url));
    if !rejected.is_empty() {
        plan.failed.push(format!("skipped {} non-http(s) URL(s)", rejected.len()));
    }
    let urls: Vec<String> = urls.into_iter().take(RESUME_MAX_URLS).collect();
    if urls.is_empty() {
        return;
    }
    let mac = mac_open(probe, Some("Firefox"), urls.clone()).or_else(|| mac_open(probe, None, urls.clone()));
    if let Some((program, args)) = mac {
        // `open` takes every URL at once; without -a it uses the default browser.
        plan.commands.push(LaunchCommand {
            target: LaunchTarget::Browser,
            program,
            args,
            cwd: None,
            restores: RestoredCounts { urls: urls.len(), ..Default::default() },
        });
    } else if let Some(firefox) = probe.which("firefox") {
        let mut args = vec!["--new-window".to_string(), urls[0].clone()];
        for url in &urls[1..] {
            args.push("--new-tab".into());
            args.push(url.clone());
        }
        plan.commands.push(LaunchCommand {
            target: LaunchTarget::Browser,
            program: firefox,
            args,
            cwd: None,
            restores: RestoredCounts { urls: urls.len(), ..Default::default() },
        });
    } else if let Some(opener) = probe.which("xdg-open") {
        for url in urls {
            plan.commands.push(LaunchCommand {
                target: LaunchTarget::Browser,
                program: opener.clone(),
                args: vec![url],
                cwd: None,
                restores: RestoredCounts { urls: 1, ..Default::default() },
            });
        }
    } else {
        plan.failed.push("no browser launcher found (tried firefox, xdg-open)".into());
    }
}

fn terminal_args(name: &str, mode: RestoreMode, cwd: &str) -> Vec<String> {
    let tab = mode == RestoreMode::Continue;
    let mut args: Vec<String> = Vec::new();
    match name {
        "gnome-terminal" | "xfce4-terminal" | "kgx" => {
            if tab {
                args.push("--tab".into());
            }
            args.push(format!("--working-directory={cwd}"));
        }
        "ptyxis" => {
            args.push(if tab { "--tab" } else { "--new-window" }.into());
            args.push(format!("--working-directory={cwd}"));
        }
        "konsole" => {
            if tab {
                args.push("--new-tab".into());
            }
            args.push("--workdir".into());
            args.push(cwd.into());
        }
        // Debian alternative with unknown flags: rely on the process cwd only.
        _ => {}
    }
    args
}

fn plan_terminal(shell: &ShellResume, mode: RestoreMode, probe: &dyn SystemProbe, plan: &mut RestorePlan) {
    let Some(cwd) = shell.cwd.as_deref().filter(|cwd| !cwd.is_empty()) else {
        return;
    };
    let expanded = expand_user(Path::new(cwd));
    if !probe.is_dir(&expanded) {
        plan.failed.push(format!("terminal working directory not found: {}", expanded.display()));
        return;
    }
    let resolved = resolve_lenient(&expanded);
    let dir = resolved.to_string_lossy().into_owned();
    if let Some((program, args)) = MAC_TERMINALS.iter().find_map(|app| mac_open(probe, Some(app), vec![dir.clone()])) {
        // `open -a Terminal <dir>` opens a new window in that directory.
        plan.commands.push(LaunchCommand {
            target: LaunchTarget::Terminal,
            program,
            args,
            cwd: Some(resolved),
            restores: RestoredCounts { shell: true, ..Default::default() },
        });
        return;
    }
    let Some((name, program)) = TERMINALS.iter().find_map(|name| probe.which(name).map(|path| (*name, path))) else {
        plan.failed.push(format!("no terminal emulator found (tried {})", TERMINALS.join(", ")));
        return;
    };
    // last_cmd is deliberately ignored: it may be sensitive or destructive.
    plan.commands.push(LaunchCommand {
        target: LaunchTarget::Terminal,
        program,
        args: terminal_args(name, mode, &resolved.to_string_lossy()),
        cwd: Some(resolved),
        restores: RestoredCounts { shell: true, ..Default::default() },
    });
}

/// Launch every planned command detached from the daemon.
pub fn execute(plan: &RestorePlan) -> RestoreResult {
    execute_with(plan, spawn_detached)
}

/// [`execute`] with an injectable spawner (tests).
pub fn execute_with<F>(plan: &RestorePlan, mut spawn: F) -> RestoreResult
where
    F: FnMut(&LaunchCommand) -> std::io::Result<()>,
{
    let mut restored = RestoredCounts::default();
    let mut failed = plan.failed.clone();
    for command in &plan.commands {
        match spawn(command) {
            Ok(()) => {
                restored.files += command.restores.files;
                restored.urls += command.restores.urls;
                restored.shell |= command.restores.shell;
            }
            Err(err) => failed.push(format!("{} launch failed: {err}", command.target.label())),
        }
    }
    RestoreResult { ok: failed.is_empty(), restored, failed }
}

/// Spawn with no shell, null stdio, in a new session; a background thread reaps
/// the child so no zombie is left behind.
pub fn spawn_detached(launch: &LaunchCommand) -> std::io::Result<()> {
    let mut command = Command::new(&launch.program);
    command.args(&launch.args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if let Some(cwd) = &launch.cwd {
        command.current_dir(cwd);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid is async-signal-safe and touches no Rust state.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = command.spawn()?;
    std::thread::Builder::new().name("intent-restore-reaper".into()).spawn(move || {
        let _ = child.wait();
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    #[derive(Default)]
    struct FakeProbe {
        programs: HashMap<&'static str, PathBuf>,
        files: HashSet<PathBuf>,
        dirs: HashSet<PathBuf>,
        env: HashMap<&'static str, String>,
        titles: Option<String>,
        macos: bool,
        mac_apps: HashSet<&'static str>,
    }

    impl FakeProbe {
        fn with_programs(names: &[&'static str]) -> Self {
            let mut probe = FakeProbe::default();
            for name in names {
                probe.programs.insert(name, PathBuf::from(format!("/usr/bin/{name}")));
            }
            probe
        }
    }

    impl SystemProbe for FakeProbe {
        fn which(&self, program: &str) -> Option<PathBuf> {
            self.programs.get(program).cloned()
        }
        fn is_file(&self, path: &Path) -> bool {
            self.files.contains(path)
        }
        fn is_dir(&self, path: &Path) -> bool {
            self.dirs.contains(path)
        }
        fn env(&self, key: &str) -> Option<String> {
            self.env.get(key).cloned()
        }
        fn window_titles(&self) -> Option<String> {
            self.titles.clone()
        }
        fn is_macos(&self) -> bool {
            self.macos
        }
        fn has_mac_app(&self, name: &str) -> bool {
            self.mac_apps.contains(name)
        }
    }

    fn mac_probe(programs: &[&'static str], apps: &[&'static str], files: &[&str]) -> FakeProbe {
        let mut probe = probe_with_files(programs, files);
        probe.macos = true;
        probe.mac_apps = apps.iter().copied().collect();
        probe
    }

    fn args(cmd: &LaunchCommand) -> Vec<&str> {
        cmd.args.iter().map(String::as_str).collect()
    }

    #[test]
    fn macos_opens_apps_that_are_not_on_path() {
        let probe = mac_probe(&["open"], &["Visual Studio Code", "Firefox", "Terminal"], &["a.rs"]);
        let plan = plan_restore_with(
            &payload(&["a.rs"], &["https://a.com/", "https://b.com/"], Some(ROOT)),
            RestoreMode::Resume,
            &probe,
        );
        assert!(plan.failed.is_empty(), "{:?}", plan.failed);
        let progs: Vec<_> = plan.commands.iter().map(|c| c.program.clone()).collect();
        assert!(progs.iter().all(|p| p == Path::new("/usr/bin/open")), "{progs:?}");
        let fa = file("a.rs");
        assert_eq!(args(&plan.commands[0]), ["-a", "Visual Studio Code", fa.as_str()]);
        assert_eq!(args(&plan.commands[1]), ["-a", "Firefox", "https://a.com/", "https://b.com/"]);
        assert_eq!(plan.commands[1].restores.urls, 2);
        assert_eq!(args(&plan.commands[2]), ["-a", "Terminal", ROOT]);
    }

    #[test]
    fn macos_prefers_the_code_command_and_the_default_browser() {
        let probe = mac_probe(&["open", "code"], &["iTerm", "Terminal"], &["a.rs"]);
        let plan = plan_restore_with(&payload(&["a.rs"], &["https://a.com/"], Some(ROOT)), RestoreMode::Resume, &probe);
        assert!(plan.failed.is_empty(), "{:?}", plan.failed);
        assert_eq!(plan.commands[0].program, PathBuf::from("/usr/bin/code"));
        assert_eq!(args(&plan.commands[1]), ["https://a.com/"]);
        assert_eq!(args(&plan.commands[2]), ["-a", "iTerm", ROOT]);
    }

    #[test]
    fn linux_never_uses_open() {
        let probe = probe_with_files(&["open", "xdg-open", "gnome-terminal"], &[]);
        let plan = plan_restore_with(&payload(&[], &["https://a.com/"], Some(ROOT)), RestoreMode::Resume, &probe);
        assert_eq!(plan.commands[0].program, PathBuf::from("/usr/bin/xdg-open"));
        assert_eq!(plan.commands[1].program, PathBuf::from("/usr/bin/gnome-terminal"));
    }

    const ROOT: &str = "/nonexistent-intent";

    fn file(name: &str) -> String {
        format!("{ROOT}/{name}")
    }

    fn payload(files: &[&str], urls: &[&str], cwd: Option<&str>) -> ResumePayload {
        ResumePayload {
            files: files.iter().map(|f| file(f)).collect(),
            urls: urls.iter().map(|u| u.to_string()).collect(),
            shell: ShellResume { cwd: cwd.map(str::to_owned), last_cmd: Some("rm -rf /".into()) },
        }
    }

    fn probe_with_files(programs: &[&'static str], files: &[&str]) -> FakeProbe {
        let mut probe = FakeProbe::with_programs(programs);
        for name in files {
            probe.files.insert(PathBuf::from(file(name)));
        }
        probe.dirs.insert(PathBuf::from(ROOT));
        probe
    }

    #[test]
    fn resume_plans_editor_browser_terminal() {
        let probe = probe_with_files(&["code", "firefox", "gnome-terminal"], &["a.rs", "b.rs"]);
        let plan = plan_restore_with(
            &payload(&["a.rs", "b.rs", "a.rs"], &["https://a.com/", "http://b.com/x"], Some(ROOT)),
            RestoreMode::Resume,
            &probe,
        );
        assert!(plan.failed.is_empty(), "{:?}", plan.failed);
        assert_eq!(plan.commands.len(), 3);
        let editor = &plan.commands[0];
        assert_eq!(editor.program, PathBuf::from("/usr/bin/code"));
        assert_eq!(editor.args, vec!["--reuse-window".to_string(), file("a.rs"), file("b.rs")]);
        assert_eq!(editor.restores.files, 2);
        let browser = &plan.commands[1];
        assert_eq!(browser.args, ["--new-window", "https://a.com/", "--new-tab", "http://b.com/x"]);
        assert_eq!(browser.restores.urls, 2);
        let terminal = &plan.commands[2];
        assert_eq!(terminal.args, [format!("--working-directory={ROOT}")]);
        assert_eq!(terminal.cwd.as_deref(), Some(Path::new(ROOT)));
        assert!(terminal.restores.shell);
        // last_cmd never reaches any argv.
        assert!(plan.commands.iter().all(|c| c.args.iter().all(|a| !a.contains("rm -rf"))));
    }

    #[test]
    fn continue_mode_uses_tabs_and_dedupes_open_files() {
        let mut probe = probe_with_files(&["codium", "firefox", "gnome-terminal"], &["a.rs", "b.rs"]);
        probe.titles = Some("0x01  0 host a.rs - project - VSCodium\n".into());
        let plan = plan_restore_with(&payload(&["a.rs", "b.rs"], &[], Some(ROOT)), RestoreMode::Continue, &probe);
        assert_eq!(plan.already_open, vec![file("a.rs")]);
        assert_eq!(plan.commands[0].program, PathBuf::from("/usr/bin/codium"));
        assert_eq!(plan.commands[0].args, vec!["--reuse-window".to_string(), file("b.rs")]);
        assert_eq!(plan.commands[1].args, ["--tab".to_string(), format!("--working-directory={ROOT}")]);
    }

    #[test]
    fn continue_mode_without_window_list_skips_dedupe() {
        let probe = probe_with_files(&["code"], &["a.rs"]);
        let plan = plan_restore_with(&payload(&["a.rs"], &[], None), RestoreMode::Continue, &probe);
        assert!(plan.already_open.is_empty());
        assert_eq!(plan.commands[0].restores.files, 1);
        // Resume mode never consults the window list.
        let mut probe = probe_with_files(&["code"], &["a.rs"]);
        probe.titles = Some("a.rs".into());
        let plan = plan_restore_with(&payload(&["a.rs"], &[], None), RestoreMode::Resume, &probe);
        assert!(plan.already_open.is_empty());
    }

    #[test]
    fn missing_files_are_skipped_and_reported() {
        let probe = probe_with_files(&["cursor"], &["a.rs"]);
        let plan = plan_restore_with(&payload(&["a.rs", "gone.rs"], &[], None), RestoreMode::Resume, &probe);
        assert_eq!(plan.failed, vec![format!("files not found: {}", file("gone.rs"))]);
        assert_eq!(plan.commands[0].program, PathBuf::from("/usr/bin/cursor"));
        assert_eq!(plan.commands[0].args, vec!["--reuse-window".to_string(), file("a.rs")]);
        let plan = plan_restore_with(&payload(&["gone.rs"], &[], None), RestoreMode::Resume, &probe);
        assert!(plan.commands.is_empty());
    }

    #[test]
    fn visual_fallback_and_no_editor() {
        let mut probe = probe_with_files(&["gvim"], &["a.rs"]);
        probe.env.insert("VISUAL", "gvim --remote-tab".into());
        let plan = plan_restore_with(&payload(&["a.rs"], &[], None), RestoreMode::Resume, &probe);
        assert_eq!(plan.commands[0].program, PathBuf::from("/usr/bin/gvim"));
        assert_eq!(plan.commands[0].args, vec!["--remote-tab".to_string(), file("a.rs")]);
        let probe = probe_with_files(&[], &["a.rs"]);
        let plan = plan_restore_with(&payload(&["a.rs"], &[], None), RestoreMode::Resume, &probe);
        assert!(plan.commands.is_empty());
        assert_eq!(plan.failed, ["no editor found (tried code, codium, cursor, $VISUAL)"]);
    }

    #[test]
    fn browser_fallback_and_url_filtering() {
        let probe = probe_with_files(&["xdg-open"], &[]);
        let plan = plan_restore_with(
            &payload(&[], &["https://a.com/", "file:///etc/passwd", "javascript:x", "https://b.com/"], None),
            RestoreMode::Resume,
            &probe,
        );
        assert_eq!(plan.commands.len(), 2);
        assert!(plan.commands.iter().all(|c| c.program == Path::new("/usr/bin/xdg-open") && c.args.len() == 1));
        assert_eq!(plan.commands[1].args, ["https://b.com/"]);
        assert_eq!(plan.failed, ["skipped 2 non-http(s) URL(s)"]);
        let plan =
            plan_restore_with(&payload(&[], &["https://a.com/"], None), RestoreMode::Resume, &FakeProbe::default());
        assert_eq!(plan.failed, ["no browser launcher found (tried firefox, xdg-open)"]);
    }

    #[test]
    fn caps_files_and_urls() {
        let names: Vec<String> = (0..8).map(|i| format!("f{i}.rs")).collect();
        let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let urls: Vec<String> = (0..12).map(|i| format!("https://h{i}.com/")).collect();
        let url_refs: Vec<&str> = urls.iter().map(String::as_str).collect();
        let probe = probe_with_files(&["code", "firefox"], &name_refs);
        let plan = plan_restore_with(&payload(&name_refs, &url_refs, None), RestoreMode::Resume, &probe);
        assert_eq!(plan.commands[0].restores.files, 5);
        assert_eq!(plan.commands[1].restores.urls, 8);
    }

    #[test]
    fn terminal_detection_and_flags() {
        for (name, resume, cont) in [
            (
                "gnome-terminal",
                vec![format!("--working-directory={ROOT}")],
                vec!["--tab".to_string(), format!("--working-directory={ROOT}")],
            ),
            (
                "ptyxis",
                vec!["--new-window".to_string(), format!("--working-directory={ROOT}")],
                vec!["--tab".to_string(), format!("--working-directory={ROOT}")],
            ),
            (
                "kgx",
                vec![format!("--working-directory={ROOT}")],
                vec!["--tab".to_string(), format!("--working-directory={ROOT}")],
            ),
            (
                "konsole",
                vec!["--workdir".to_string(), ROOT.to_string()],
                vec!["--new-tab".to_string(), "--workdir".to_string(), ROOT.to_string()],
            ),
            (
                "xfce4-terminal",
                vec![format!("--working-directory={ROOT}")],
                vec!["--tab".to_string(), format!("--working-directory={ROOT}")],
            ),
            ("x-terminal-emulator", vec![], vec![]),
        ] {
            let probe = probe_with_files(&[name], &[]);
            let plan = plan_restore_with(&payload(&[], &[], Some(ROOT)), RestoreMode::Resume, &probe);
            assert_eq!(plan.commands[0].args, resume, "{name}");
            assert_eq!(plan.commands[0].cwd.as_deref(), Some(Path::new(ROOT)));
            let plan = plan_restore_with(&payload(&[], &[], Some(ROOT)), RestoreMode::Continue, &probe);
            assert_eq!(plan.commands[0].args, cont, "{name}");
        }
        // Detection order prefers gnome-terminal.
        let probe = probe_with_files(&["konsole", "gnome-terminal"], &[]);
        let plan = plan_restore_with(&payload(&[], &[], Some(ROOT)), RestoreMode::Resume, &probe);
        assert_eq!(plan.commands[0].program, PathBuf::from("/usr/bin/gnome-terminal"));
    }

    #[test]
    fn terminal_failures() {
        let probe = probe_with_files(&["gnome-terminal"], &[]);
        let plan = plan_restore_with(&payload(&[], &[], Some("/nonexistent-intent/nope")), RestoreMode::Resume, &probe);
        assert_eq!(plan.failed, ["terminal working directory not found: /nonexistent-intent/nope"]);
        let probe = probe_with_files(&[], &[]);
        let plan = plan_restore_with(&payload(&[], &[], Some(ROOT)), RestoreMode::Resume, &probe);
        assert!(plan.failed[0].starts_with("no terminal emulator found"));
        let plan = plan_restore_with(&payload(&[], &[], Some("")), RestoreMode::Resume, &probe);
        assert!(plan.failed.is_empty() && plan.commands.is_empty());
    }

    #[test]
    fn execute_counts_successes_and_failures() {
        let probe = probe_with_files(&["code", "xdg-open", "kgx"], &["a.rs"]);
        let plan = plan_restore_with(
            &payload(&["a.rs"], &["https://a.com/", "https://b.com/"], Some(ROOT)),
            RestoreMode::Resume,
            &probe,
        );
        let mut calls = 0;
        let result = execute_with(&plan, |command| {
            calls += 1;
            if command.args.first().map(String::as_str) == Some("https://b.com/") {
                Err(std::io::Error::other("boom"))
            } else {
                Ok(())
            }
        });
        assert_eq!(calls, 4);
        assert_eq!(result.restored, RestoredCounts { files: 1, urls: 1, shell: true });
        assert_eq!(result.failed, ["browser launch failed: boom"]);
        assert!(!result.ok);
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["restored"], serde_json::json!({"files": 1, "urls": 1, "shell": true}));
        let empty = execute_with(&RestorePlan::default(), |_| Ok(()));
        assert!(empty.ok);
    }

    #[test]
    fn request_validation() {
        let request = |value: serde_json::Value| serde_json::from_value::<RestoreRequest>(value).unwrap().validate();
        let (payload, mode) =
            request(serde_json::json!({"files": ["/a", "/a"], "urls": ["https://x/"], "shell": {"cwd": "/"}})).unwrap();
        assert_eq!(mode, RestoreMode::Resume);
        assert_eq!(payload.files, ["/a"]);
        assert_eq!(request(serde_json::json!({"mode": "continue"})).unwrap().1, RestoreMode::Continue);
        let err = |value: serde_json::Value| request(value).unwrap_err().message;
        assert_eq!(err(serde_json::json!({"mode": "replay"})), "mode must be resume or continue");
        assert_eq!(
            err(serde_json::json!({"files": ["1", "2", "3", "4", "5", "6"]})),
            "at most five files may be restored"
        );
        let urls: Vec<String> = (0..9).map(|i| format!("https://h{i}/")).collect();
        assert_eq!(err(serde_json::json!({"urls": urls})), "at most eight URLs may be restored");
        assert_eq!(err(serde_json::json!({"urls": ["ftp://x/"]})), "urls must use http or https");
        assert_eq!(err(serde_json::json!({"urls": ["https://"]})), "urls must use http or https");
        assert_eq!(
            err(serde_json::json!({"shell": {"last_cmd": "x".repeat(501)}})),
            "shell.last_cmd must be at most 500 characters"
        );
    }

    #[test]
    fn find_executable_searches_path() {
        let dir = tempfile::tempdir().unwrap();
        let tool = dir.path().join("intent-fake-tool");
        std::fs::write(&tool, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(find_executable("intent-fake-tool", Some(dir.path().as_os_str())), None);
            std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(find_executable("intent-fake-tool", Some(dir.path().as_os_str())), Some(tool.clone()));
        assert_eq!(find_executable("missing-tool", Some(dir.path().as_os_str())), None);
        assert_eq!(find_executable("intent-fake-tool", None), None);
    }

    #[cfg(unix)]
    #[test]
    fn spawn_detached_runs_without_shell() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("marker");
        let launch = LaunchCommand {
            target: LaunchTarget::Terminal,
            program: PathBuf::from("/usr/bin/touch"),
            args: vec![marker.to_string_lossy().into_owned()],
            cwd: Some(dir.path().to_path_buf()),
            restores: RestoredCounts::default(),
        };
        spawn_detached(&launch).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(marker.exists());
        let missing = LaunchCommand { program: PathBuf::from("/nonexistent-intent/bin"), ..launch };
        assert!(spawn_detached(&missing).is_err());
    }

    #[test]
    fn x11_detection() {
        let mut probe = FakeProbe::default();
        assert!(!is_x11_session(&probe));
        probe.env.insert("DISPLAY", ":0".into());
        assert!(is_x11_session(&probe));
        probe.env.insert("WAYLAND_DISPLAY", "wayland-0".into());
        assert!(!is_x11_session(&probe));
        probe.env.insert("XDG_SESSION_TYPE", "x11".into());
        assert!(is_x11_session(&probe));
        probe.env.insert("XDG_SESSION_TYPE", "wayland".into());
        assert!(!is_x11_session(&probe));
    }
}
