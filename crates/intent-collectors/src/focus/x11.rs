//! Event-driven X11 focus backend (pure-Rust `x11rb`, no xdotool/xprop forks).
//!
//! Subscribes to `PropertyNotify` on the root window (`_NET_ACTIVE_WINDOW`) and on
//! the currently active window (`_NET_WM_NAME` / `WM_NAME` / `WM_CLASS`). The
//! connection socket is registered with tokio, so the task sleeps until the X
//! server sends something — zero polling.

use std::os::fd::{AsRawFd, RawFd};

use tokio::io::Interest;
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, EventMask, Window};
use x11rb::rust_connection::RustConnection;

use super::{BackendEnd, FocusSample, parse_wm_class};
use crate::session::SessionEnv;
use crate::status::StatusHandle;

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        _NET_ACTIVE_WINDOW,
        _NET_WM_NAME,
        UTF8_STRING,
    }
}

/// 1024 32-bit units = 4 KiB, enough for a 512-character UTF-8 title.
const TITLE_LONG_LENGTH: u32 = 1024;

struct Tracker {
    conn: RustConnection,
    root: Window,
    atoms: Atoms,
    active: Option<Window>,
}

impl AsRawFd for Tracker {
    fn as_raw_fd(&self) -> RawFd {
        self.conn.stream().as_raw_fd()
    }
}

impl Tracker {
    fn new(conn: RustConnection, screen: usize) -> Result<Self, String> {
        let root = conn.setup().roots.get(screen).ok_or("x11: screen not found")?.root;
        let atoms = Atoms::new(&conn)
            .map_err(|e| format!("x11: intern atoms: {e}"))?
            .reply()
            .map_err(|e| format!("x11: intern atoms: {e}"))?;
        conn.change_window_attributes(root, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE))
            .map_err(|e| format!("x11: select root events: {e}"))?
            .check()
            .map_err(|e| format!("x11: select root events: {e}"))?;
        Ok(Self { conn, root, atoms, active: None })
    }

    fn property(&self, win: Window, atom: impl Into<u32>, long_length: u32) -> Result<(u32, Vec<u8>), ReplyError> {
        let reply = self.conn.get_property(false, win, atom, AtomEnum::ANY, 0, long_length)?.reply()?;
        Ok((reply.type_, reply.value))
    }

    fn active_window(&self) -> Option<Window> {
        let reply = self
            .conn
            .get_property(false, self.root, self.atoms._NET_ACTIVE_WINDOW, AtomEnum::WINDOW, 0, 1)
            .ok()?
            .reply()
            .ok()?;
        reply.value32()?.next().filter(|w| *w != 0)
    }

    fn set_mask(&self, win: Window, mask: EventMask) {
        // Unchecked: a window that vanished just yields an ignored X error event.
        let _ = self.conn.change_window_attributes(win, &ChangeWindowAttributesAux::new().event_mask(mask));
    }

    /// Re-read `_NET_ACTIVE_WINDOW`, move the title subscription, return a sample.
    fn refresh_active(&mut self) -> Option<FocusSample> {
        let new = self.active_window();
        if new != self.active {
            if let Some(old) = self.active {
                self.set_mask(old, EventMask::NO_EVENT);
            }
            if let Some(w) = new {
                self.set_mask(w, EventMask::PROPERTY_CHANGE);
            }
            self.active = new;
        }
        self.sample(new?)
    }

    fn title(&self, win: Window) -> Option<String> {
        if let Ok((ty, bytes)) = self.property(win, self.atoms._NET_WM_NAME, TITLE_LONG_LENGTH)
            && ty == self.atoms.UTF8_STRING
            && !bytes.is_empty()
        {
            return Some(String::from_utf8_lossy(&bytes).into_owned());
        }
        let (ty, bytes) = self.property(win, AtomEnum::WM_NAME, TITLE_LONG_LENGTH).ok()?;
        Some(if ty == u32::from(AtomEnum::STRING) {
            // ICCCM STRING is Latin-1.
            bytes.iter().map(|&b| b as char).collect()
        } else {
            String::from_utf8_lossy(&bytes).into_owned()
        })
    }

    fn sample(&self, win: Window) -> Option<FocusSample> {
        let (_, class) = self.property(win, AtomEnum::WM_CLASS, 256).ok()?;
        let title = self.title(win).unwrap_or_default();
        Some(FocusSample::new(win.to_string(), parse_wm_class(&class), &title))
    }

    fn handle(&mut self, event: Event) -> Option<FocusSample> {
        match event {
            Event::PropertyNotify(e) if e.window == self.root && e.atom == self.atoms._NET_ACTIVE_WINDOW => {
                self.refresh_active()
            }
            Event::PropertyNotify(e)
                if Some(e.window) == self.active
                    && (e.atom == self.atoms._NET_WM_NAME
                        || e.atom == u32::from(AtomEnum::WM_NAME)
                        || e.atom == u32::from(AtomEnum::WM_CLASS)) =>
            {
                self.sample(e.window)
            }
            // BadWindow from racing a destroyed window, etc.
            _ => None,
        }
    }
}

pub(super) async fn run(
    env: &SessionEnv,
    tx: mpsc::Sender<FocusSample>,
    status: &StatusHandle,
    cancel: &CancellationToken,
) -> BackendEnd {
    let (conn, screen) = match x11rb::connect(env.display.as_deref()) {
        Ok(c) => c,
        Err(e) => return BackendEnd::NotAvailable(format!("x11: cannot connect to display: {e}")),
    };
    let tracker = match Tracker::new(conn, screen) {
        Ok(t) => t,
        Err(e) => return BackendEnd::NotAvailable(e),
    };
    // SAFETY: the AsyncFd owns the Tracker, which owns the X connection and therefore
    // the socket; it stays open and unchanged for the AsyncFd's whole lifetime.
    let mut fd = match unsafe { AsyncFd::register_with_interest(tracker, Interest::READABLE) } {
        Ok(fd) => fd,
        Err(e) => return BackendEnd::NotAvailable(format!("x11: register socket: {e}")),
    };
    status.set_running("x11");
    let tracker = fd.get_mut();
    if tracker.active_window().is_none() {
        status.set_error(Some("window manager has not published _NET_ACTIVE_WINDOW yet".into()));
    }
    if let Some(sample) = tracker.refresh_active()
        && tx.send(sample).await.is_err()
    {
        return BackendEnd::Stopped;
    }

    loop {
        let tracker = fd.get_mut();
        // Drain everything already buffered (including events read while waiting for replies).
        loop {
            match tracker.conn.poll_for_event() {
                Ok(Some(event)) => {
                    if let Some(sample) = tracker.handle(event) {
                        status.set_error(None);
                        if tx.send(sample).await.is_err() {
                            return BackendEnd::Stopped;
                        }
                    }
                }
                Ok(None) => break,
                Err(e) => return BackendEnd::Lost(format!("x11 connection lost: {e}")),
            }
        }
        if let Err(e) = tracker.conn.flush() {
            return BackendEnd::Lost(format!("x11 connection lost: {e}"));
        }
        tokio::select! {
            _ = cancel.cancelled() => return BackendEnd::Stopped,
            ready = fd.readable() => match ready {
                Ok(mut guard) => guard.clear_ready(),
                Err(e) => return BackendEnd::Lost(format!("x11 socket error: {e}")),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In an X11 session (or under Xvfb + an EWMH WM, e.g. `xvfb-run -a openbox`):
    /// `cargo test -p intent-collectors -- --ignored x11_live --nocapture`
    #[test]
    #[ignore = "needs an X11 display"]
    fn x11_live() {
        let (conn, screen) = x11rb::connect(None).expect("DISPLAY");
        let mut t = Tracker::new(conn, screen).expect("tracker");
        println!("active window sample: {:?}", t.refresh_active());
    }
}
