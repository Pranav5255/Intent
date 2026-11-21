//! Intent desktop shell: overlay palette + main window + tray.
//!
//! All daemon HTTP goes through the `api` command (see `daemon.rs`); the
//! webviews never hold the bearer token.

mod accent;
mod daemon;
mod tray;

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, State, Webview, WebviewWindow, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as _};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use daemon::{ApiResponse, Daemon};

#[cfg(target_os = "macos")]
const DEFAULT_SHORTCUT: &str = "Alt+Space"; // Super+Space is Spotlight on macOS.
#[cfg(not(target_os = "macos"))]
const DEFAULT_SHORTCUT: &str = "Super+Space";

/// A shortcut press this soon after the palette hid on blur closes it (X11 grab).
const BLUR_TOGGLE_WINDOW: Duration = Duration::from_millis(250);

pub struct AppState {
    pub daemon: Daemon,
    shortcut: Mutex<String>,
    /// Route requested before the main webview was ready to listen.
    pending_route: Mutex<Option<String>>,
    /// When the palette last hid because it lost focus (see `toggle_overlay`).
    overlay_blurred_at: Mutex<Option<Instant>>,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
async fn api(
    app: AppHandle,
    state: State<'_, AppState>,
    method: String,
    path: String,
    body: Option<Value>,
) -> Result<ApiResponse, String> {
    let response = state.daemon.request(&method, &path, body.filter(|b| !b.is_null())).await?;
    // A pause/resume from one window (e.g. the palette) must reach the tray and
    // the other windows now, not at their next poll.
    if method.eq_ignore_ascii_case("POST") && path == "/v1/capture/pause" && (200..300).contains(&response.status) {
        tray::refresh(&app).await;
    }
    Ok(response)
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
    focus_when_shown(&win, false);
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
    // On X11 the shortcut's own key grab takes focus from the palette, so the
    // blur handler has usually hidden it a moment before this press arrives.
    // That press meant "close", not "open again".
    let just_blurred = app
        .state::<AppState>()
        .overlay_blurred_at
        .lock()
        .ok()
        .and_then(|mut t| t.take())
        .is_some_and(|t| t.elapsed() < BLUR_TOGGLE_WINDOW);
    if just_blurred {
        return;
    }
    let _ = place_overlay(app, &win);
    let _ = win.show();
    focus_when_shown(&win, true);
    let _ = win.emit("overlay-shown", ());
}

/// Focus a window and its webview once it is mapped. On Linux `set_focus` is
/// ignored while the window is still hidden (`show` is applied asynchronously),
/// so calling it right after `show` never raised or focused anything; it only
/// worked where the window manager happens to focus new windows. `reposition`
/// re-applies the palette position: on its first map the X11 window manager's
/// own placement wins over the position set while it was hidden.
fn focus_when_shown(win: &WebviewWindow, reposition: bool) {
    let win = win.clone();
    tauri::async_runtime::spawn(async move {
        for _ in 0..50 {
            if win.is_visible().unwrap_or(false) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let _ = win.set_focus();
        let _ = AsRef::<Webview>::as_ref(&win).set_focus();
        if reposition {
            // Before the first map, X11 can report a 0×0 size (centring the left edge,
            // x = 640 on a 1280 px screen) and the window manager's placement can land
            // after ours. Re-place until the window sits where it should, for up to 1 s.
            for _ in 0..20 {
                if !win.is_visible().unwrap_or(false) {
                    break;
                }
                let placed = place_overlay(win.app_handle(), &win);
                tokio::time::sleep(Duration::from_millis(50)).await;
                if placed.is_some() && placed == win.outer_position().ok() {
                    break;
                }
            }
        }
    });
}

/// Place the palette and return the position it was given, or `None` when its size
/// is not known yet (an unmapped X11 window can report 0×0).
fn place_overlay(app: &AppHandle, win: &WebviewWindow) -> Option<PhysicalPosition<i32>> {
    let monitor = app
        .cursor_position()
        .ok()
        .and_then(|p| app.monitor_from_point(p.x, p.y).ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten());
    let (Some(monitor), Ok(size)) = (monitor, win.outer_size()) else {
        let _ = win.center();
        return None;
    };
    if size.width <= 1 || size.height <= 1 {
        return None;
    }
    let area = monitor.work_area();
    let position = overlay_position(area.position, area.size, size);
    let _ = win.set_position(position);
    Some(position)
}

/// Centred horizontally in the work area, a fifth of the way down.
fn overlay_position(
    origin: PhysicalPosition<i32>,
    area: tauri::PhysicalSize<u32>,
    window: tauri::PhysicalSize<u32>,
) -> PhysicalPosition<i32> {
    let x = origin.x + (area.width as i32 - window.width as i32) / 2;
    let y = origin.y + (area.height as i32 / 5).max(0);
    PhysicalPosition::new(x, y)
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
    !id.is_empty()
        && id.len() <= 128
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == ':')
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
            overlay_blurred_at: Mutex::new(None),
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
                let app = handle.clone();
                overlay.on_window_event(move |event| {
                    if let WindowEvent::Focused(false) = event {
                        if w.is_visible().unwrap_or(false)
                            && let Ok(mut at) = app.state::<AppState>().overlay_blurred_at.lock()
                        {
                            *at = Some(Instant::now());
                        }
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

    #[test]
    fn overlay_is_centred_in_the_work_area() {
        let p = overlay_position(
            PhysicalPosition::new(0, 32),
            tauri::PhysicalSize::new(1280, 768),
            tauri::PhysicalSize::new(920, 540),
        );
        assert_eq!((p.x, p.y), (180, 32 + 153));
    }

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
