//! `org.gnome.Mutter.IdleMonitor` backend (GNOME, Wayland or X11). Event-driven:
//! idle watches at 5s / 60s (feed buckets) and at the threshold, plus a
//! one-shot user-active watch armed whenever the user is idle.

use std::collections::HashMap;

use futures_util::StreamExt;
use zbus::Connection;
use zbus::proxy::CacheProperties;

use super::{BackendEnd, IdleTracker, emit};
use crate::CollectorContext;
use crate::feed::ActivityFeed;

const BACKEND: &str = "mutter-idle-monitor";
const NAME: &str = "org.gnome.Mutter.IdleMonitor";

#[zbus::proxy(
    interface = "org.gnome.Mutter.IdleMonitor",
    default_service = "org.gnome.Mutter.IdleMonitor",
    default_path = "/org/gnome/Mutter/IdleMonitor/Core",
    gen_blocking = false
)]
trait IdleMonitor {
    fn get_idletime(&self) -> zbus::Result<u64>;
    fn add_idle_watch(&self, interval: u64) -> zbus::Result<u32>;
    fn add_user_active_watch(&self) -> zbus::Result<u32>;
    fn remove_watch(&self, id: u32) -> zbus::Result<()>;
    #[zbus(signal)]
    fn watch_fired(&self, id: u32) -> zbus::Result<()>;
}

/// Watches registered with the current Mutter instance.
struct Watches {
    idle: HashMap<u32, u64>,
    active: Option<u32>,
}

async fn install(proxy: &IdleMonitorProxy<'_>, threshold_ms: u64) -> zbus::Result<Watches> {
    let mut intervals = vec![5_000u64, 60_000, threshold_ms];
    intervals.sort_unstable();
    intervals.dedup();
    let mut idle = HashMap::new();
    for ms in intervals {
        idle.insert(proxy.add_idle_watch(ms).await?, ms);
    }
    Ok(Watches { idle, active: None })
}

pub(super) async fn run(
    threshold_ms: u64,
    feed: &ActivityFeed,
    ctx: &CollectorContext,
) -> BackendEnd {
    let conn = match Connection::session().await {
        Ok(c) => c,
        Err(e) => return BackendEnd::NotAvailable(format!("mutter: no session bus: {e}")),
    };
    let proxy = match IdleMonitorProxy::builder(&conn)
        .cache_properties(CacheProperties::No)
        .build()
        .await
    {
        Ok(p) => p,
        Err(e) => return BackendEnd::NotAvailable(format!("mutter: proxy: {e}")),
    };
    let (mut owners, mut fired) = match (
        proxy.inner().receive_owner_changed().await,
        proxy.receive_watch_fired().await,
    ) {
        (Ok(o), Ok(f)) => (o, f),
        (Err(e), _) | (_, Err(e)) => {
            return BackendEnd::NotAvailable(format!("mutter: subscribe: {e}"));
        }
    };
    let has_owner = match zbus::fdo::DBusProxy::new(&conn).await {
        Ok(dbus) => dbus
            .name_has_owner(NAME.try_into().expect("valid bus name"))
            .await
            .unwrap_or(false),
        Err(_) => false,
    };
    if !has_owner {
        return BackendEnd::NotAvailable(format!(
            "{NAME} is not on the session bus (not a GNOME session)"
        ));
    }

    let mut tracker = IdleTracker::new(threshold_ms);
    let mut watches: Option<Watches> = None;
    let mut need_setup = true;

    loop {
        if need_setup {
            need_setup = false;
            match install(&proxy, threshold_ms).await {
                Ok(mut w) => {
                    ctx.status.set_running(BACKEND);
                    let idle_ms = proxy.get_idletime().await.unwrap_or(0);
                    feed.set_idle_ms(idle_ms);
                    if idle_ms >= 1_000 {
                        w.active = proxy.add_user_active_watch().await.ok();
                    }
                    watches = Some(w);
                    if let Some(t) = tracker.observe(idle_ms)
                        && emit(&ctx.sink, t).await.is_err()
                    {
                        return BackendEnd::Stopped;
                    }
                }
                Err(e) => ctx.status.set_waiting(
                    Some(BACKEND.into()),
                    format!("mutter: add watches failed: {e}"),
                ),
            }
        }
        tokio::select! {
            _ = ctx.cancel.cancelled() => {
                if let Some(w) = watches.take() {
                    for id in w.idle.keys().copied().chain(w.active) {
                        let _ = proxy.remove_watch(id).await;
                    }
                }
                return BackendEnd::Stopped;
            }
            owner = owners.next() => match owner {
                // gnome-shell restarted (X11 `Alt+F2 r`) — old watches are gone.
                Some(Some(_)) => need_setup = true,
                Some(None) => {
                    watches = None;
                    ctx.status.set_waiting(Some(BACKEND.into()), "Mutter left the session bus");
                }
                None => return BackendEnd::Lost("mutter: session bus connection closed".into()),
            },
            sig = fired.next() => {
                let Some(sig) = sig else {
                    return BackendEnd::Lost("mutter: session bus connection closed".into());
                };
                let Ok(args) = sig.args() else { continue };
                let id = *args.id();
                let Some(w) = watches.as_mut() else { continue };
                let observed = if w.active == Some(id) {
                    // User-active watches are one-shot on Mutter's side.
                    w.active = None;
                    Some(0)
                } else if let Some(&ms) = w.idle.get(&id) {
                    if w.active.is_none() {
                        w.active = proxy.add_user_active_watch().await.ok();
                    }
                    Some(ms)
                } else {
                    None
                };
                if let Some(idle_ms) = observed {
                    feed.set_idle_ms(idle_ms);
                    if let Some(t) = tracker.observe(idle_ms)
                        && emit(&ctx.sink, t).await.is_err()
                    {
                        return BackendEnd::Stopped;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In a GNOME session: `cargo test -p intent-collectors -- --ignored mutter_live`
    #[tokio::test]
    #[ignore = "needs a GNOME session bus"]
    async fn mutter_live() {
        let conn = Connection::session().await.unwrap();
        let proxy = IdleMonitorProxy::new(&conn).await.unwrap();
        let idle = proxy.get_idletime().await.unwrap();
        println!("GetIdletime -> {idle} ms");
        let id = proxy.add_idle_watch(1_000).await.unwrap();
        proxy.remove_watch(id).await.unwrap();
    }
}
