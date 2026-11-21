//! Small helpers shared by routes and jobs.

use intent_store::{Store, now_ts};
use jiff::civil::Date;

/// Today's calendar day in the store's day zone.
pub fn today(store: &Store) -> String {
    store.local_day(now_ts())
}

/// The calendar day before `date` (`YYYY-MM-DD`); falls back to `date` on bad input.
pub fn previous_day(date: &str) -> String {
    date.parse::<Date>()
        .ok()
        .and_then(|d| d.yesterday().ok())
        .map(|d| d.to_string())
        .unwrap_or_else(|| date.to_string())
}

/// Yesterday in the store's day zone (DST-safe: derived from today's date).
pub fn yesterday(store: &Store) -> String {
    previous_day(&today(store))
}

/// A real `YYYY-MM-DD` calendar date (1.x `_validated_date`).
pub fn is_valid_date(value: &str) -> bool {
    value.parse::<Date>().is_ok_and(|d| d.to_string() == value)
}

/// Local hour of day (0-23) in the host's zone.
pub fn local_hour() -> u32 {
    u32::try_from(jiff::Zoned::now().hour()).unwrap_or(0)
}

/// Size in bytes of the database file plus its WAL/SHM side files.
pub fn database_size(path: &std::path::Path) -> u64 {
    let mut total = 0;
    for suffix in ["", "-wal", "-shm"] {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        if let Ok(meta) = std::fs::metadata(std::path::PathBuf::from(p)) {
            total += meta.len();
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previous_day_handles_month_and_year_edges() {
        assert_eq!(previous_day("2026-03-01"), "2026-02-28");
        assert_eq!(previous_day("2026-01-01"), "2025-12-31");
        assert!(is_valid_date("2024-02-29"));
        assert!(!is_valid_date("2026-02-30"));
        assert!(!is_valid_date("2026-2-3"));
    }
}
