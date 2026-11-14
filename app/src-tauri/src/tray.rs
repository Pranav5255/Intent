//! Tray icon. Pause state is always read FROM the daemon (polled), never
//! assumed locally, so the tray, overlay footer and main-window banner agree.

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use intent_types::CurrentIntent;
use serde::Deserialize;
use serde_json::json;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, Wry};
use tauri_plugin_notification::NotificationExt;

use crate::{AppState, show_main};

const POLL: Duration = Duration::from_secs(20);

#[derive(Debug, Default, Clone, PartialEq, Deserialize)]
struct Status {
    #[serde(default)]
    capture_paused: bool,
    #[serde(default)]
    capture_paused_until: Option<i64>,
}

pub struct TrayItems {
    now: MenuItem<Wry>,
    pause15: MenuItem<Wry>,
    pause60: MenuItem<Wry>,
    resume: MenuItem<Wry>,
    last: Mutex<Option<(Status, Option<String>)>>,
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let now = MenuItem::with_id(app, "now", "Now: checking…", true, None::<&str>)?;
    let open = MenuItem::with_id(app, "open", "Open Intent", true, None::<&str>)?;
    let pause15 = MenuItem::with_id(app, "pause15", "Pause for 15 minutes", true, None::<&str>)?;
    let pause60 = MenuItem::with_id(app, "pause60", "Pause for 1 hour", true, None::<&str>)?;
    let resume = MenuItem::with_id(app, "resume", "Resume capture", false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Intent", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &now,
            &PredefinedMenuItem::separator(app)?,
            &open,
            &PredefinedMenuItem::separator(app)?,
            &pause15,
            &pause60,
            &resume,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id("intent")
        .tooltip("Intent")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            let app = app.clone();
            match event.id().as_ref() {
                "open" => show_main(&app, Some("today".into())),
                "now" => show_main(&app, Some("today".into())),
                "pause15" => spawn_pause(app, Some(15 * 60)),
                "pause60" => spawn_pause(app, Some(60 * 60)),
                "resume" => spawn_pause(app, None),
                "quit" => app.exit(0),
                _ => {}
            }
        });
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }
    builder.build(app)?;

    app.manage(TrayItems { now, pause15, pause60, resume, last: Mutex::new(None) });

    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            refresh(&handle).await;
            tokio::time::sleep(POLL).await;
        }
    });
    Ok(())
}

fn unix_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn resumes_in(ts: i64) -> String {
    // Relative ("in 42m") avoids pulling in a timezone database for the tray.
    let mins = ((ts - unix_now()).max(0) + 59) / 60;
    if mins >= 60 { format!("in {}h {:02}m", mins / 60, mins % 60) } else { format!("in {mins}m") }
}

fn spawn_pause(app: AppHandle, seconds: Option<i64>) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let body = match seconds {
            Some(s) => json!({ "paused": true, "until": unix_now() + s }),
            None => json!({ "paused": false, "until": null }),
        };
        let result = state.daemon.post("/v1/capture/pause", body).await;
        let (title, text) = match (&result, seconds) {
            (Ok(()), Some(s)) => ("Capture paused", format!("Nothing is recorded for the next {} minutes.", s / 60)),
            (Ok(()), None) => ("Capture resumed", "Intent is recording again.".to_string()),
            (Err(e), _) => ("Couldn’t change capture", e.clone()),
        };
        let _ = app.notification().builder().title(title).body(text).show();
        refresh(&app).await;
    });
}

/// Poll the daemon and update menu labels; tell webviews when pause state changes.
pub async fn refresh(app: &AppHandle) {
    let state = app.state::<AppState>();
    let status: Option<Status> = state.daemon.get("/v1/status").await;
    let current: Option<CurrentIntent> = state.daemon.get::<Option<CurrentIntent>>("/intents/current").await.flatten();
    let Some(items) = app.try_state::<TrayItems>() else { return };

    let label = match (&status, &current) {
        (None, _) => "Intent isn’t running".to_string(),
        (Some(s), _) if s.capture_paused => match s.capture_paused_until {
            Some(until) => format!("Paused · resumes {}", resumes_in(until)),
            None => "Paused".to_string(),
        },
        (Some(_), Some(c)) => format!("Now: {}", ellipsize(&c.label, 48)),
        (Some(_), None) => "Now: nothing yet".to_string(),
    };
    let paused = status.as_ref().is_some_and(|s| s.capture_paused);
    let _ = items.now.set_text(&label);
    let _ = items.pause15.set_enabled(status.is_some() && !paused);
    let _ = items.pause60.set_enabled(status.is_some() && !paused);
    let _ = items.resume.set_enabled(paused);
    if let Some(tray) = app.tray_by_id("intent") {
        let _ = tray.set_tooltip(Some(&format!("Intent · {label}")));
    }

    let snapshot = (status.clone().unwrap_or_default(), current.map(|c| c.label));
    let changed = {
        let mut last = items.last.lock().expect("tray state");
        let changed = last.as_ref() != Some(&snapshot);
        *last = Some(snapshot);
        changed
    };
    if changed {
        let _ = app.emit("capture-changed", ());
    }
}

fn ellipsize(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max - 1).collect::<String>())
    }
}
