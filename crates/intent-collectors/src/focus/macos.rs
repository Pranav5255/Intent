//! macOS focus backend: the frontmost window from the window server (`CGWindowList`).
//!
//! The window server lists on-screen windows front to back. The first normal window
//! (layer 0) belongs to the active app. Reading the app name needs no permission.
//! Window titles need the Screen Recording permission; without it the title is empty
//! and the status says so.
//!
//! A process without an AppKit run loop gets no app-switch notifications, so this
//! backend samples the list on the activity feed's interval: every 2 s while the user
//! is active, up to 10 s while idle. One sample costs about a millisecond.

use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_graphics::{
    CGPreflightScreenCaptureAccess, CGWindowListCopyWindowInfo, CGWindowListOption, kCGNullWindowID, kCGWindowLayer,
    kCGWindowName, kCGWindowNumber, kCGWindowOwnerName, kCGWindowOwnerPID,
};
use tokio::sync::mpsc;

use super::{BackendEnd, FocusSample, friendly_app_name};
use crate::CollectorContext;
use crate::feed::ActivityFeed;

const BACKEND: &str = "macos-window-server";

/// One entry of the window server's list, in front-to-back order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub number: i64,
    pub layer: i64,
    pub pid: i64,
    pub owner: String,
    pub title: Option<String>,
}

/// Window owners that are part of the system UI, not an app the user works in.
const SYSTEM_OWNERS: &[&str] = &["Window Server", "Dock", "Control Center", "SystemUIServer", "Notification Center"];

/// The window the user is working in: the first normal window of a real app.
pub fn frontmost(windows: &[WindowInfo]) -> Option<&WindowInfo> {
    windows.iter().find(|w| w.layer == 0 && !w.owner.is_empty() && !SYSTEM_OWNERS.contains(&w.owner.as_str()))
}

/// Map a macOS app name to the names the Linux backends report.
pub fn app_from_macos(owner: &str) -> String {
    let lower = owner.trim().to_lowercase();
    let mapped = match lower.as_str() {
        "code" | "visual studio code" => "code",
        "google chrome" => "google-chrome",
        "terminal" => "terminal",
        "iterm2" | "iterm" => "iterm2",
        _ => return friendly_app_name(&lower.replace(' ', "-")),
    };
    mapped.to_owned()
}

pub fn sample_of(w: &WindowInfo) -> FocusSample {
    // Window numbers are per window; the app name keeps titles of different apps apart.
    FocusSample::new(w.number.to_string(), app_from_macos(&w.owner), w.title.as_deref().unwrap_or(""))
}

fn number(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<i64> {
    dict.get(key)?.downcast::<CFNumber>().ok()?.as_i64()
}

fn string(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<String> {
    Some(dict.get(key)?.downcast::<CFString>().ok()?.to_string())
}

/// The on-screen windows, front to back. `None` when the window server does not answer.
fn list_windows() -> Option<Vec<WindowInfo>> {
    let options = CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let raw = CGWindowListCopyWindowInfo(options, kCGNullWindowID)?;
    // SAFETY: CGWindowListCopyWindowInfo documents an array of dictionaries with
    // CFString keys. Every value is read through a checked downcast.
    let list: CFRetained<CFArray<CFDictionary<CFString, CFType>>> = unsafe { CFRetained::cast_unchecked(raw) };
    // SAFETY: the keys are immutable constants exported by CoreGraphics.
    let (k_number, k_layer, k_pid, k_owner, k_name) =
        unsafe { (kCGWindowNumber, kCGWindowLayer, kCGWindowOwnerPID, kCGWindowOwnerName, kCGWindowName) };
    let mut out = Vec::with_capacity(list.len());
    for i in 0..list.len() {
        let Some(dict) = list.get(i) else { continue };
        out.push(WindowInfo {
            number: number(&dict, k_number).unwrap_or(0),
            layer: number(&dict, k_layer).unwrap_or(-1),
            pid: number(&dict, k_pid).unwrap_or(0),
            owner: string(&dict, k_owner).unwrap_or_default(),
            title: string(&dict, k_name).filter(|t| !t.is_empty()),
        });
    }
    Some(out)
}

fn current() -> Option<Option<FocusSample>> {
    let windows = list_windows()?;
    Some(frontmost(&windows).map(sample_of))
}

pub(super) async fn run(
    tx: mpsc::Sender<FocusSample>,
    feed: Option<&ActivityFeed>,
    ctx: &CollectorContext,
) -> BackendEnd {
    match tokio::task::spawn_blocking(current).await {
        Ok(Some(_)) => {}
        Ok(None) => return BackendEnd::NotAvailable("macos: the window server did not answer".into()),
        Err(e) => return BackendEnd::NotAvailable(format!("macos: {e}")),
    }
    ctx.status.set_running(BACKEND);
    let titles = CGPreflightScreenCaptureAccess();
    ctx.status.set_detail("window_titles", titles);
    if !titles {
        ctx.status.set_detail(
            "hint",
            "Window titles need the Screen Recording permission (System Settings > Privacy & Security).",
        );
    }
    loop {
        match tokio::task::spawn_blocking(current).await {
            Ok(Some(Some(sample))) => {
                if tx.send(sample).await.is_err() {
                    return BackendEnd::Stopped;
                }
            }
            Ok(Some(None)) => {}
            Ok(None) => return BackendEnd::Lost("macos: the window server stopped answering".into()),
            Err(e) => return BackendEnd::Lost(format!("macos: {e}")),
        }
        let delay = feed.map_or(std::time::Duration::from_secs(2), ActivityFeed::recommended_poll_interval);
        tokio::select! {
            _ = ctx.cancel.cancelled() => return BackendEnd::Stopped,
            _ = tokio::time::sleep(delay) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(number: i64, layer: i64, owner: &str, title: Option<&str>) -> WindowInfo {
        WindowInfo { number, layer, pid: 1, owner: owner.into(), title: title.map(Into::into) }
    }

    #[test]
    fn frontmost_skips_menus_and_system_ui() {
        let list = [
            w(1, 25, "SystemUIServer", None),
            w(2, 0, "Dock", None),
            w(3, 0, "Code", Some("main.rs — Intent")),
            w(4, 0, "Firefox", Some("Docs")),
        ];
        assert_eq!(frontmost(&list).map(|x| x.number), Some(3));
        assert_eq!(frontmost(&[w(1, 24, "Menubar", None)]), None);
        assert_eq!(frontmost(&[]), None);
    }

    #[test]
    fn mac_app_names_match_linux_names() {
        assert_eq!(app_from_macos("Code"), "code");
        assert_eq!(app_from_macos("Google Chrome"), "google-chrome");
        assert_eq!(app_from_macos("Firefox"), "firefox");
        assert_eq!(app_from_macos("iTerm2"), "iterm2");
        assert_eq!(app_from_macos("Microsoft Word"), "microsoft-word");
    }

    #[test]
    fn a_window_without_title_permission_has_an_empty_title() {
        let s = sample_of(&w(7, 0, "Safari", None));
        assert_eq!((s.window_id.as_str(), s.app.as_str(), s.title.as_str()), ("7", "safari", ""));
    }

    /// On a Mac: `cargo test -p intent-collectors -- --ignored macos_focus_live --nocapture`
    #[test]
    #[ignore = "needs a macOS login session"]
    fn macos_focus_live() {
        println!("{:?}", current());
    }
}
