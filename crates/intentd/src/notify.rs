//! "Continue yesterday?" desktop notification (Linux, freedesktop notifications).
//!
//! Shown at most once per day, in the morning, only while the user is at the
//! machine and capture is not paused. Its action opens the Intent app on the
//! intent's preview (`intent --preview <id>`); nothing is restored without the
//! user confirming in the app. Disable with `INTENT_NOTIFICATIONS=0`.

use std::time::Duration;

use notify_rust::{Notification, Timeout};

use crate::state::Shared;
use crate::util;

const KEY: &str = "continue_notified_date";
const ACTION: &str = "continue";

pub async fn maybe_continue_yesterday(state: &Shared) {
    if std::env::var("INTENT_NOTIFICATIONS").is_ok_and(|v| v.trim() == "0") {
        return;
    }
    if !(5..12).contains(&util::local_hour()) || state.pause.is_paused() || state.feed.idle_ms() > 120_000 {
        return;
    }
    let store = state.store.clone();
    let today = util::today(&store);
    let yesterday = util::previous_day(&today);
    let pick = tokio::task::spawn_blocking(move || {
        if store.get_metadata(KEY).ok().flatten().as_deref() == Some(today.as_str()) {
            return None;
        }
        // Mark first: never nag twice, even if showing fails.
        let _ = store.set_metadata(KEY, &today);
        let intents = store.intents_for_date(&yesterday).ok()?;
        let digest = intent_engine::digest::build_digest(&intents, &yesterday);
        let id = digest.top_intent_ids.first()?.clone();
        let label = intents.iter().find(|i| i.id == id).map(|i| i.label.clone())?;
        Some((id, label))
    })
    .await
    .ok()
    .flatten();
    let Some((id, label)) = pick else { return };
    tokio::task::spawn_blocking(move || show(&id, &label));
}

fn show(intent_id: &str, label: &str) {
    let handle = Notification::new()
        .appname("Intent")
        .summary("Continue yesterday?")
        .body(&format!("Pick up where you left off: {label}"))
        .icon("intent")
        .action(ACTION, "Continue")
        .action("default", "Continue")
        .timeout(Timeout::Milliseconds(Duration::from_secs(60).as_millis() as u32))
        .show();
    match handle {
        Ok(handle) => handle.wait_for_action(|action| {
            if action == ACTION || action == "default" {
                let spawned = std::process::Command::new("intent")
                    .args(["--preview", intent_id])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
                match spawned {
                    // Reap in the background so no zombie is left.
                    Ok(mut child) => {
                        std::thread::spawn(move || {
                            let _ = child.wait();
                        });
                    }
                    Err(err) => tracing::warn!(error = %err, "could not launch the Intent app"),
                }
            }
        }),
        Err(err) => tracing::debug!(error = %err, "desktop notification unavailable"),
    }
}
