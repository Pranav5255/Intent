//! The user service that runs `intentd`: a systemd user unit on Linux, a launchd
//! agent on macOS.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

const SYSTEMD_UNIT: &str = "intentd.service";
/// launchd label, and the agent's file name in `~/Library/LaunchAgents`.
pub const LAUNCHD_LABEL: &str = "io.github.pranav5255.intentd";

pub fn enable() -> Result<()> {
    if cfg!(target_os = "macos") {
        launchd_enable()
    } else {
        let _ = systemctl(&["import-environment", "DISPLAY", "XAUTHORITY", "WAYLAND_DISPLAY", "XDG_SESSION_TYPE"]);
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", "--now", SYSTEMD_UNIT])
    }
}

pub fn disable() -> Result<()> {
    if cfg!(target_os = "macos") { launchd_disable() } else { systemctl(&["disable", "--now", SYSTEMD_UNIT]) }
}

/// `active`, `inactive`, `failed`, … as the service manager reports it.
pub fn state() -> String {
    if cfg!(target_os = "macos") {
        let out = Command::new("launchctl").args(["print", &launchd_target()]).output();
        match out {
            Ok(o) if o.status.success() => launchd_state(&String::from_utf8_lossy(&o.stdout)),
            Ok(_) => "inactive".into(),
            Err(_) => "unknown".into(),
        }
    } else {
        Command::new("systemctl")
            .args(["--user", "is-active", SYSTEMD_UNIT])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|_| "unknown".into())
    }
}

fn systemctl(args: &[&str]) -> Result<()> {
    let status = Command::new("systemctl").arg("--user").args(args).status().context("running systemctl")?;
    if !status.success() {
        bail!("systemctl --user {} failed", args.join(" "));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// launchd
// ---------------------------------------------------------------------------

fn uid() -> Result<String> {
    let out = Command::new("id").arg("-u").output().context("running id -u")?;
    let uid = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if uid.is_empty() || !uid.bytes().all(|b| b.is_ascii_digit()) {
        bail!("id -u returned {uid:?}");
    }
    Ok(uid)
}

fn launchd_domain() -> String {
    uid().map_or_else(|_| "gui/501".into(), |uid| format!("gui/{uid}"))
}

fn launchd_target() -> String {
    format!("{}/{LAUNCHD_LABEL}", launchd_domain())
}

pub fn plist_path(home: &Path) -> PathBuf {
    home.join("Library").join("LaunchAgents").join(format!("{LAUNCHD_LABEL}.plist"))
}

/// `intentd` next to this `intentctl` (Homebrew, the app bundle), else on `$PATH`.
fn daemon_path() -> Result<PathBuf> {
    let sibling = std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.join("intentd")));
    if let Some(path) = sibling.filter(|p| p.is_file()) {
        return Ok(path);
    }
    std::env::var_os("PATH")
        .and_then(|paths| std::env::split_paths(&paths).map(|d| d.join("intentd")).find(|p| p.is_file()))
        .context("intentd not found next to intentctl or on PATH")
}

fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The launchd agent. `PATH` includes Homebrew so Resume finds `code` and friends;
/// launchd starts agents with only the system directories.
pub fn launchd_plist(daemon: &Path, log_dir: &Path) -> String {
    let daemon = xml_escape(&daemon.to_string_lossy());
    let log = xml_escape(&log_dir.join("intentd.log").to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LAUNCHD_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{daemon}</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>PATH</key>
    <string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <dict>
    <key>SuccessfulExit</key>
    <false/>
  </dict>
  <key>ThrottleInterval</key>
  <integer>5</integer>
  <key>ProcessType</key>
  <string>Background</string>
  <key>LimitLoadToSessionType</key>
  <string>Aqua</string>
  <key>StandardOutPath</key>
  <string>{log}</string>
  <key>StandardErrorPath</key>
  <string>{log}</string>
</dict>
</plist>
"#
    )
}

/// `state = running` from `launchctl print`.
pub fn launchd_state(print: &str) -> String {
    let state = print.lines().find_map(|l| l.trim().strip_prefix("state = ")).map(str::trim);
    match state {
        Some("running") => "active".into(),
        Some(other) => other.into(),
        None => "unknown".into(),
    }
}

fn launchctl(args: &[&str]) -> Result<()> {
    let status = Command::new("launchctl").args(args).status().context("running launchctl")?;
    if !status.success() {
        bail!("launchctl {} failed", args.join(" "));
    }
    Ok(())
}

fn launchd_enable() -> Result<()> {
    let home = super::home();
    let plist = plist_path(&home);
    let logs = home.join("Library").join("Logs").join("Intent");
    std::fs::create_dir_all(&logs).with_context(|| format!("creating {}", logs.display()))?;
    if let Some(dir) = plist.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    std::fs::write(&plist, launchd_plist(&daemon_path()?, &logs))
        .with_context(|| format!("writing {}", plist.display()))?;
    // A loaded agent keeps its old definition: unload it first, then load the new one.
    let _ = Command::new("launchctl").args(["bootout", &launchd_target()]).output();
    let plist_arg = plist.to_string_lossy();
    launchctl(&["bootstrap", &launchd_domain(), &plist_arg])
}

fn launchd_disable() -> Result<()> {
    let _ = Command::new("launchctl").args(["bootout", &launchd_target()]).output();
    let plist = plist_path(&super::home());
    match std::fs::remove_file(&plist) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("removing {}", plist.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_runs_the_daemon_at_login_and_logs_to_library() {
        let p = launchd_plist(Path::new("/opt/homebrew/bin/intentd"), Path::new("/Users/a&b/Library/Logs/Intent"));
        assert!(p.contains("<string>/opt/homebrew/bin/intentd</string>"));
        assert!(p.contains("<key>RunAtLoad</key>\n  <true/>"));
        assert!(p.contains("/Users/a&amp;b/Library/Logs/Intent/intentd.log"));
        assert!(p.contains(&format!("<string>{LAUNCHD_LABEL}</string>")));
    }

    #[test]
    fn launchctl_print_state() {
        assert_eq!(launchd_state("gui/501/x = {\n\tactive count = 1\n\tstate = running\n"), "active");
        assert_eq!(launchd_state("\tstate = not running\n"), "not running");
        assert_eq!(launchd_state(""), "unknown");
    }

    #[test]
    fn plist_lives_in_launch_agents() {
        assert_eq!(
            plist_path(Path::new("/Users/u")),
            PathBuf::from("/Users/u/Library/LaunchAgents/io.github.pranav5255.intentd.plist")
        );
    }
}
