//! Graphical-session detection (pure; fed from the environment).
//!
//! `intentd` runs as a `systemd --user` service; GNOME imports `DISPLAY`,
//! `WAYLAND_DISPLAY`, `XDG_SESSION_TYPE` and `XDG_CURRENT_DESKTOP` into the user
//! manager when the session starts, so the service must be ordered after
//! `graphical-session.target` for these to be present.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    X11,
    Wayland,
    /// A macOS login session (the window server).
    MacOs,
    /// TTY / headless / unknown.
    Unknown,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SessionEnv {
    pub session_type: Option<String>,
    pub wayland_display: Option<String>,
    pub display: Option<String>,
    pub current_desktop: Option<String>,
    /// Running on macOS, where the window server replaces X11 and Wayland.
    pub macos: bool,
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.trim().is_empty())
}

impl SessionEnv {
    pub fn from_env() -> Self {
        let get = |k: &str| non_empty(std::env::var(k).ok());
        Self {
            session_type: get("XDG_SESSION_TYPE"),
            wayland_display: get("WAYLAND_DISPLAY"),
            display: get("DISPLAY"),
            current_desktop: get("XDG_CURRENT_DESKTOP"),
            macos: cfg!(target_os = "macos"),
        }
    }

    /// Wayland wins whenever there is any sign of it: under Wayland `DISPLAY`
    /// usually points at XWayland, which only sees X11 clients and would
    /// report wrong focus and idle data.
    pub fn kind(&self) -> SessionKind {
        if self.macos {
            return SessionKind::MacOs;
        }
        let ty = self.session_type.as_deref().map(str::to_ascii_lowercase);
        match ty.as_deref() {
            Some("wayland") => return SessionKind::Wayland,
            Some("x11") if self.wayland_display.is_none() => return SessionKind::X11,
            _ => {}
        }
        if self.wayland_display.is_some() {
            SessionKind::Wayland
        } else if self.display.is_some() {
            SessionKind::X11
        } else {
            SessionKind::Unknown
        }
    }

    /// `XDG_CURRENT_DESKTOP` is a colon-separated list, e.g. `ubuntu:GNOME`.
    pub fn is_gnome(&self) -> bool {
        self.current_desktop.as_deref().is_some_and(|d| d.split(':').any(|p| p.eq_ignore_ascii_case("gnome")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(ty: Option<&str>, wl: Option<&str>, x: Option<&str>, de: Option<&str>) -> SessionEnv {
        SessionEnv {
            session_type: ty.map(Into::into),
            wayland_display: wl.map(Into::into),
            display: x.map(Into::into),
            current_desktop: de.map(Into::into),
            macos: false,
        }
    }

    #[test]
    fn wayland_beats_xwayland_display() {
        let e = env(Some("wayland"), Some("wayland-0"), Some(":0"), Some("ubuntu:GNOME"));
        assert_eq!(e.kind(), SessionKind::Wayland);
        assert!(e.is_gnome());
        assert_eq!(env(None, Some("wayland-0"), Some(":0"), None).kind(), SessionKind::Wayland);
        assert_eq!(env(Some("x11"), Some("wayland-0"), Some(":0"), None).kind(), SessionKind::Wayland);
    }

    #[test]
    fn x11_and_unknown() {
        assert_eq!(env(Some("x11"), None, Some(":0"), Some("KDE")).kind(), SessionKind::X11);
        assert_eq!(env(None, None, Some(":1"), None).kind(), SessionKind::X11);
        assert_eq!(env(Some("tty"), None, None, None).kind(), SessionKind::Unknown);
        assert!(!env(None, None, None, Some("KDE")).is_gnome());
    }
}
