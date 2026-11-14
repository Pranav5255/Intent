//! GNOME Shell (Wayland + X11) focus backend: talks to the bundled
//! `intent-focus@intent.local` extension over the session bus.
//!
//! D-Bus contract (see `gnome-extension/intent-focus@intent.local/extension.js`):
//!
//! ```text
//! name      org.intent.Focus
//! path      /org/intent/Focus
//! interface org.intent.Focus
//!   method  GetFocus() -> (s window_id, s app_id, s wm_class, s title, u pid)
//!   method  GetVersion() -> (u version)
//!   signal  FocusChanged(s window_id, s app_id, s wm_class, s title, u pid)
//! ```
//!
//! `window_id` is empty when nothing is focused (overview, desktop). The
//! extension owns the name only while enabled; GNOME disables extensions on
//! the lock screen, so the name vanishing is normal and we just wait for it.

use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use zbus::Connection;
use zbus::proxy::CacheProperties;

use super::{BackendEnd, FocusSample, app_from_gnome};
use crate::status::StatusHandle;

pub const DBUS_NAME: &str = "org.intent.Focus";
pub const DBUS_PATH: &str = "/org/intent/Focus";
pub const DBUS_INTERFACE: &str = "org.intent.Focus";
const EXTENSION_UUID: &str = "intent-focus@intent.local";
const BACKEND: &str = "gnome-extension";

#[zbus::proxy(
    interface = "org.intent.Focus",
    default_service = "org.intent.Focus",
    default_path = "/org/intent/Focus",
    gen_blocking = false
)]
trait IntentFocus {
    fn get_focus(&self) -> zbus::Result<(String, String, String, String, u32)>;

    #[zbus(signal)]
    fn focus_changed(
        &self,
        window_id: String,
        app_id: String,
        wm_class: String,
        title: String,
        pid: u32,
    ) -> zbus::Result<()>;
}

fn to_sample(window_id: &str, app_id: &str, wm_class: &str, title: &str) -> Option<FocusSample> {
    if window_id.is_empty() {
        return None;
    }
    Some(FocusSample::new(window_id, app_from_gnome(app_id, wm_class), title))
}

async fn current(proxy: &IntentFocusProxy<'_>) -> Option<FocusSample> {
    let (id, app_id, wm_class, title, _pid) = proxy.get_focus().await.ok()?;
    to_sample(&id, &app_id, &wm_class, &title)
}

fn waiting_reason() -> String {
    format!(
        "waiting for GNOME Shell extension {EXTENSION_UUID} (install it and run `gnome-extensions enable {EXTENSION_UUID}`; it is also inactive while the screen is locked)"
    )
}

pub(super) async fn run(
    wait: bool,
    tx: mpsc::Sender<FocusSample>,
    status: &StatusHandle,
    cancel: &CancellationToken,
) -> BackendEnd {
    let conn = match Connection::session().await {
        Ok(c) => c,
        Err(e) => return BackendEnd::NotAvailable(format!("gnome: no session bus: {e}")),
    };
    let proxy = match IntentFocusProxy::builder(&conn).cache_properties(CacheProperties::No).build().await {
        Ok(p) => p,
        Err(e) => return BackendEnd::NotAvailable(format!("gnome: proxy: {e}")),
    };
    // Subscribe before checking the owner so no transition is missed.
    let (mut owners, mut signals) =
        match (proxy.inner().receive_owner_changed().await, proxy.receive_focus_changed().await) {
            (Ok(o), Ok(s)) => (o, s),
            (Err(e), _) | (_, Err(e)) => {
                return BackendEnd::NotAvailable(format!("gnome: subscribe: {e}"));
            }
        };
    let has_owner = match zbus::fdo::DBusProxy::new(&conn).await {
        Ok(dbus) => dbus.name_has_owner(DBUS_NAME.try_into().expect("valid bus name")).await.unwrap_or(false),
        Err(_) => false,
    };

    if has_owner {
        status.set_running(BACKEND);
        if let Some(s) = current(&proxy).await
            && tx.send(s).await.is_err()
        {
            return BackendEnd::Stopped;
        }
    } else if wait {
        status.set_waiting(Some(BACKEND.into()), waiting_reason());
    } else {
        return BackendEnd::NotAvailable(format!("GNOME Shell extension {EXTENSION_UUID} is not running"));
    }

    loop {
        tokio::select! {
            _ = cancel.cancelled() => return BackendEnd::Stopped,
            owner = owners.next() => match owner {
                Some(Some(_)) => {
                    status.set_running(BACKEND);
                    if let Some(s) = current(&proxy).await
                        && tx.send(s).await.is_err()
                    {
                        return BackendEnd::Stopped;
                    }
                }
                Some(None) => status.set_waiting(Some(BACKEND.into()), waiting_reason()),
                None => return BackendEnd::Lost("gnome: session bus connection closed".into()),
            },
            signal = signals.next() => match signal {
                Some(sig) => {
                    let Ok(a) = sig.args() else { continue };
                    if let Some(s) = to_sample(a.window_id(), a.app_id(), a.wm_class(), a.title())
                        && tx.send(s).await.is_err()
                    {
                        return BackendEnd::Stopped;
                    }
                }
                None => return BackendEnd::Lost("gnome: session bus connection closed".into()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_window_is_ignored() {
        assert!(to_sample("", "firefox.desktop", "firefox", "x").is_none());
        let s = to_sample("42", "firefox_firefox.desktop", "firefox_firefox", "Docs").unwrap();
        assert_eq!((s.window_id.as_str(), s.app.as_str(), s.title.as_str()), ("42", "firefox", "Docs"));
    }

    /// Run in a GNOME session with the extension enabled:
    /// `cargo test -p intent-collectors -- --ignored gnome_extension_live`
    #[tokio::test]
    #[ignore = "needs a GNOME session with intent-focus@intent.local enabled"]
    async fn gnome_extension_live() {
        let conn = Connection::session().await.unwrap();
        let proxy = IntentFocusProxy::new(&conn).await.unwrap();
        let focus = proxy.get_focus().await.unwrap();
        println!("GetFocus -> {focus:?}");
        assert!(!focus.0.is_empty(), "expected a focused window");
    }
}
