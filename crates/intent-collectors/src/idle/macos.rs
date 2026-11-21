//! macOS idle backend: seconds since the last keyboard, mouse or trackpad input,
//! from the HID system state. Needs no permission.

use objc2_core_graphics::{CGEventSource, CGEventSourceStateID, CGEventType};

use super::{BackendEnd, IdleTracker, emit, x11_poll_delay};
use crate::CollectorContext;
use crate::feed::ActivityFeed;

const BACKEND: &str = "macos-hid";

/// `kCGAnyInputEventType`: any keyboard, mouse or tablet event.
const ANY_INPUT: CGEventType = CGEventType(u32::MAX);

/// Milliseconds since the last user input. Negative or NaN readings count as active.
pub fn idle_ms_from_seconds(seconds: f64) -> u64 {
    if seconds.is_finite() && seconds > 0.0 { (seconds * 1000.0) as u64 } else { 0 }
}

fn idle_ms() -> u64 {
    idle_ms_from_seconds(CGEventSource::seconds_since_last_event_type(CGEventSourceStateID::HIDSystemState, ANY_INPUT))
}

pub(super) async fn run(threshold_ms: u64, feed: &ActivityFeed, ctx: &CollectorContext) -> BackendEnd {
    ctx.status.set_running(BACKEND);
    let mut tracker = IdleTracker::new(threshold_ms);
    loop {
        let idle_ms = idle_ms();
        feed.set_idle_ms(idle_ms);
        if let Some(t) = tracker.observe(idle_ms)
            && emit(&ctx.sink, t).await.is_err()
        {
            return BackendEnd::Stopped;
        }
        // Same schedule as X11: wake when the threshold would be crossed, 2 s while idle.
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

    #[test]
    fn seconds_convert_to_milliseconds() {
        assert_eq!(idle_ms_from_seconds(1.5), 1_500);
        assert_eq!(idle_ms_from_seconds(-1.0), 0);
        assert_eq!(idle_ms_from_seconds(f64::NAN), 0);
    }

    /// On a Mac: `cargo test -p intent-collectors -- --ignored macos_idle_live --nocapture`
    #[test]
    #[ignore = "needs a macOS login session"]
    fn macos_idle_live() {
        println!("idle_ms = {}", idle_ms());
    }
}
