//! MIT-SCREEN-SAVER idle backend for non-GNOME X11 sessions (replaces `xprintidle` forks).

use x11rb::connection::{Connection, RequestConnection};
use x11rb::errors::ReplyError;
use x11rb::protocol::screensaver;

use super::{BackendEnd, IdleTracker, emit, x11_poll_delay};
use crate::CollectorContext;
use crate::feed::ActivityFeed;
use crate::session::SessionEnv;

const BACKEND: &str = "x11-screensaver";

pub(super) async fn run(
    env: &SessionEnv,
    threshold_ms: u64,
    feed: &ActivityFeed,
    ctx: &CollectorContext,
) -> BackendEnd {
    let (conn, screen) = match x11rb::connect(env.display.as_deref()) {
        Ok(c) => c,
        Err(e) => return BackendEnd::NotAvailable(format!("x11: cannot connect to display: {e}")),
    };
    match conn.extension_information(screensaver::X11_EXTENSION_NAME) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return BackendEnd::NotAvailable("x11: MIT-SCREEN-SAVER extension missing".into());
        }
        Err(e) => return BackendEnd::NotAvailable(format!("x11: {e}")),
    }
    let Some(root) = conn.setup().roots.get(screen).map(|s| s.root) else {
        return BackendEnd::NotAvailable("x11: screen not found".into());
    };
    ctx.status.set_running(BACKEND);
    let mut tracker = IdleTracker::new(threshold_ms);
    loop {
        let idle_ms = match screensaver::query_info(&conn, root)
            .map_err(ReplyError::from)
            .and_then(|c| c.reply())
        {
            Ok(info) => u64::from(info.ms_since_user_input),
            Err(e) => return BackendEnd::Lost(format!("x11 connection lost: {e}")),
        };
        feed.set_idle_ms(idle_ms);
        if let Some(t) = tracker.observe(idle_ms)
            && emit(&ctx.sink, t).await.is_err()
        {
            return BackendEnd::Stopped;
        }
        let delay = x11_poll_delay(idle_ms, threshold_ms, tracker.is_idle());
        tokio::select! {
            _ = ctx.cancel.cancelled() => return BackendEnd::Stopped,
            _ = tokio::time::sleep(delay) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In an X11 session: `cargo test -p intent-collectors -- --ignored x11_idle_live --nocapture`
    #[test]
    #[ignore = "needs an X11 display with MIT-SCREEN-SAVER"]
    fn x11_idle_live() {
        let (conn, screen) = x11rb::connect(None).expect("DISPLAY");
        let root = conn.setup().roots[screen].root;
        let info = screensaver::query_info(&conn, root)
            .unwrap()
            .reply()
            .unwrap();
        println!("ms_since_user_input = {}", info.ms_since_user_input);
    }
}
