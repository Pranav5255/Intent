//! Chronological grouping of normalized events into activity sessions (0.1 `sessionize.py`).

use std::ops::Range;

use intent_types::{Family, NormalizedEvent};

pub const DEFAULT_SESSION_GAP_MINUTES: i64 = 15;

/// Split ordered events into sessions on gaps `> gap_minutes` and on
/// `idle_start` / `idle_end` markers (which are themselves dropped).
///
/// Because markers only ever end a session, every session is a contiguous range
/// of `events`; ranges are returned instead of copies.
pub fn sessionize(events: &[NormalizedEvent], gap_minutes: i64) -> Vec<Range<usize>> {
    let gap_seconds = gap_minutes * 60;
    let mut sessions = Vec::new();
    let mut current: Option<Range<usize>> = None;
    for (index, event) in events.iter().enumerate() {
        if is_idle_marker(event) {
            if let Some(range) = current.take() {
                sessions.push(range);
            }
            continue;
        }
        current = match current.take() {
            None => Some(index..index + 1),
            Some(range) if event.ts - events[range.end - 1].ts > gap_seconds => {
                sessions.push(range);
                Some(index..index + 1)
            }
            Some(range) => Some(range.start..index + 1),
        };
    }
    sessions.extend(current);
    sessions
}

fn is_idle_marker(event: &NormalizedEvent) -> bool {
    event.family == Family::Idle && matches!(event.category.as_str(), "idle_start" | "idle_end")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ev;

    #[test]
    fn splits_on_gap_and_idle_markers() {
        let events = vec![
            ev("a", 0, "shell", "command"),
            ev("b", 60, "shell", "command"),
            ev("c", 60 + 15 * 60, "shell", "command"), // exactly 15 min: same session
            ev("d", 60 + 30 * 60 + 1, "shell", "command"),
            ev("i1", 60 + 30 * 60 + 2, "linux", "idle_start"),
            ev("i2", 60 + 30 * 60 + 3, "linux", "idle_end"),
            ev("e", 60 + 30 * 60 + 4, "vscode", "document_change"),
            ev("f", 60 + 30 * 60 + 5, "linux", "idle_tick"),
        ];
        assert_eq!(sessionize(&events, 15), vec![0..3, 3..4, 6..8]);
    }

    #[test]
    fn trivial_inputs() {
        assert!(sessionize(&[], 15).is_empty());
        assert_eq!(sessionize(&[ev("a", 0, "shell", "command")], 15), vec![0..1]);
        assert!(sessionize(&[ev("i", 0, "linux", "idle_start")], 15).is_empty());
    }
}
