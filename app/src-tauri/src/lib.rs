//! Intent desktop shell: overlay palette + main window + tray.
//!
//! All daemon HTTP goes through the `api` command (see `daemon.rs`); the
//! webviews never hold the bearer token.

mod accent;
mod daemon;
mod tray;

use std::path::PathBuf;
use std::sync::Mutex;

use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, State, WebviewWindow, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as _};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use daemon::{ApiResponse, Daemon};

#[cfg(target_os = "macos")]
const DEFAULT_SHORTCUT: &str = "Alt+Space"; // Super+Space is Spotlight on macOS.
#[cfg(not(target_os = "macos"))]
const DEFAULT_SHORTCUT: &str = "Super+Space";

pub struct AppState {
    pub daemon: Daemon,
    shortcut: Mutex<String>,
    /// Route requested before the main webview was ready to listen.
    pending_route: Mutex<Option<String>>,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
async fn api(state: State<'_, AppState>, method: String, path: String, body: Option<Value>) -> Result<ApiResponse, String> {
    state.daemon.request(&method, &path, body.filter(|b| !b.is_null())).await
}

#[tauri::command]
fn open_main(app: AppHandle, route: Option<String>) {
    show_main(&app, route);
}

#[tauri::command]
fn hide_overlay(app: AppHandle) {
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.hide();
    }
}

#[tauri::command]
fn take_pending_route(state: State<'_, AppState>) -> Option<String> {
    state.pending_route.lock().ok()?.take()
}

#[tauri::command]
fn get_shortcut(state: State<'_, AppState>) -> String {
    state.shortcut.lock().map(|s| s.clone()).unwrap_or_else(|_| DEFAULT_SHORTCUT.into())
}

#[tauri::command]
fn set_shortcut(app: AppHandle, state: State<'_, AppState>, accelerator: String) -> Result<(), String> {
    let next: Shortcut = accelerator.parse().map_err(|e| format!("Not a valid shortcut: {e}"))?;
    let gs = app.global_shortcut();
    let previous = state.shortcut.lock().map_err(|e| e.to_string())?.clone();
    if let Ok(prev) = previous.parse::<Shortcut>() {
        let _ = gs.unregister(prev);
    }
    if let Err(e) = gs.register(next) {
        // Put the old one back so the user is never left without a shortcut.
        if let Ok(prev) = previous.parse::<Shortcut>() {
            let _ = gs.register(prev);
        }
        return Err(format!("Another app already uses this shortcut ({e})"));
    }
    *state.shortcut.lock().map_err(|e| e.to_string())? = accelerator.clone();
    if let Some(path) = shortcut_file(&app) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, &accelerator);
    }
    Ok(())
}

#[tauri::command]
async fn get_accent() -> Option<String> {
    accent::accent_hex().await
}

#[tauri::command]
fn get_autostart(app: AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    let launcher = app.autolaunch();
    if enabled { launcher.enable() } else { launcher.disable() }.map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

/// Show the main window and route it (`today`, `timeline`, `search`,
/// `settings`, `settings:<section>` or `intent:<id>`).
pub fn show_main(app: &AppHandle, route: Option<String>) {
    let Some(win) = app.get_webview_window("main") else { return };
    if let Some(route) = route.clone()
        && let Ok(mut pending) = app.state::<AppState>().pending_route.lock()
    {
        *pending = Some(route);
    }
    let _ = win.unminimize();
    let _ = win.show();
    let _ = win.set_focus();
    if let Some(route) = route {
        let _ = win.emit("navigate", route);
    }
    if let Some(overlay) = app.get_webview_window("overlay") {
        let _ = overlay.hide();
    }
}

/// Toggle the palette, centred horizontally on the monitor under the cursor
/// and placed a fifth of the way down (where the eye already is).
fn toggle_overlay(app: &AppHandle) {
    let Some(win) = app.get_webview_window("overlay") else { return };
    if win.is_visible().unwrap_or(false) {
        let _ = win.hide();
        return;
    }
    place_overlay(app, &win);
    let _ = win.show();
    let _ = win.set_focus();
    let _ = win.emit("overlay-shown", ());
}

fn place_overlay(app: &AppHandle, win: &WebviewWindow) {
    let monitor = app
        .cursor_position()
        .ok()
        .and_then(|p| app.monitor_from_point(p.x, p.y).ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten());
    let (Some(monitor), Ok(size)) = (monitor, win.outer_size()) else {
        let _ = win.center();
        return;
    };
    let area = monitor.work_area();
    let x = area.position.x + (area.size.width as i32 - size.width as i32) / 2;
    let y = area.position.y + (area.size.height as i32 / 5).max(0);
    let _ = win.set_position(PhysicalPosition::new(x, y));
}

fn shortcut_file(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("shortcut"))
}

/// `--preview <intent_id>`, `--preview=<id>` or an `intent://preview/<id>` link.
fn preview_from_args(args: &[String]) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--preview" {
            return iter.next().cloned().filter(|id| valid_id(id));
        }
        if let Some(id) = arg.strip_prefix("--preview=") {
            return Some(id.to_string()).filter(|id| valid_id(id));
        }
        if let Some(id) = arg.strip_prefix("intent://preview/") {
            return Some(id.trim_end_matches('/').to_string()).filter(|id| valid_id(id));
        }
    }
    None
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == ':')
}

fn handle_launch(app: &AppHandle, args: &[String], first: bool) {
    if let Some(id) = preview_from_args(args) {
        show_main(app, Some(format!("intent:{id}")));
    } else if args.iter().any(|a| a == "--toggle") {
        toggle_overlay(app);
    } else if !(first && args.iter().any(|a| a == "--hidden")) {
        show_main(app, None);
    }
}

// ---------------------------------------------------------------------------
// Entry
// ---------------------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Must be first: a second launch forwards its argv here and exits (1.x had no lock).
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            handle_launch(app, &argv, false);
        }))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        toggle_overlay(app);
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec!["--hidden"])))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_window_state::Builder::default().with_denylist(&["overlay"]).build())
        .manage(AppState {
            daemon: Daemon::new(),
            shortcut: Mutex::new(DEFAULT_SHORTCUT.into()),
            pending_route: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            api,
            open_main,
            hide_overlay,
            take_pending_route,
            get_shortcut,
            set_shortcut,
            get_accent,
            get_autostart,
            set_autostart
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            // Global shortcut: saved choice, else the platform default.
            let saved = shortcut_file(&handle)
                .and_then(|p| std::fs::read_to_string(p).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| s.parse::<Shortcut>().is_ok());
            let accel = saved.unwrap_or_else(|| DEFAULT_SHORTCUT.to_string());
            match accel.parse::<Shortcut>() {
                Ok(sc) => {
                    if let Err(e) = handle.global_shortcut().register(sc) {
                        eprintln!("intent: could not register {accel}: {e}");
                    }
                }
                Err(e) => eprintln!("intent: bad shortcut {accel}: {e}"),
            }
            *app.state::<AppState>().shortcut.lock().expect("state") = accel;

            // Overlay hides on blur, like any GNOME popover.
            if let Some(overlay) = handle.get_webview_window("overlay") {
                let w = overlay.clone();
                overlay.on_window_event(move |event| {
                    if let WindowEvent::Focused(false) = event {
                        let _ = w.hide();
                    }
                });
            }
            // Closing the main window keeps Intent in the tray.
            if let Some(main) = handle.get_webview_window("main") {
                let w = main.clone();
                main.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = w.hide();
                    }
                });
            }

            tray::build(&handle)?;

            let args: Vec<String> = std::env::args().collect();
            handle_launch(&handle, &args, true);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Intent");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_preview_arguments() {
        assert_eq!(preview_from_args(&args(&["intent", "--preview", "int_7f3a04"])).as_deref(), Some("int_7f3a04"));
        assert_eq!(preview_from_args(&args(&["intent", "--preview=abc-1"])).as_deref(), Some("abc-1"));
        assert_eq!(preview_from_args(&args(&["intent", "intent://preview/xyz/"])).as_deref(), Some("xyz"));
        assert_eq!(preview_from_args(&args(&["intent", "--preview", "../etc"])), None);
        assert_eq!(preview_from_args(&args(&["intent", "--hidden"])), None);
    }
}
