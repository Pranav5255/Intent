//! Calendar-day bucketing.
//!
//! 0.1 computed an event's day from the *current* process time zone at read time, so
//! changing TZ (or running the engine and capture server under different TZs) silently
//! moved events between days. 0.2 computes the day once, at insert time, from an explicit
//! [`DayZone`] and stores it in `events.day`.

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::{Offset, TimeZone};

use crate::error::{Result, invalid};

/// Time zone used to assign events to calendar days.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DayZone {
    /// The host's local zone as reported by the OS when the event is inserted.
    #[default]
    Local,
    /// A fixed UTC offset in seconds east of UTC (no DST).
    Fixed(i32),
    /// A named IANA zone (DST-aware), e.g. `Europe/Berlin`.
    Named(TimeZone),
}

impl DayZone {
    /// Parse `local`, `UTC`, `+05:30` / `-0800` style offsets, or an IANA zone name.
    pub fn parse(spec: &str) -> Result<Self> {
        let s = spec.trim();
        if s.is_empty() || s.eq_ignore_ascii_case("local") {
            return Ok(DayZone::Local);
        }
        if s.eq_ignore_ascii_case("utc") || s == "Z" {
            return Ok(DayZone::Fixed(0));
        }
        if let Some(sign) = s.chars().next().filter(|c| *c == '+' || *c == '-') {
            let digits: String = s[1..].chars().filter(|c| *c != ':').collect();
            if digits.len() == 4 && digits.chars().all(|c| c.is_ascii_digit()) {
                let h: i32 = digits[..2].parse().unwrap_or(0);
                let m: i32 = digits[2..].parse().unwrap_or(0);
                if h <= 14 && m < 60 {
                    let secs = h * 3600 + m * 60;
                    return Ok(DayZone::Fixed(if sign == '-' { -secs } else { secs }));
                }
            }
            return Err(invalid(format!("bad UTC offset {s:?}")));
        }
        TimeZone::get(s).map(DayZone::Named).map_err(|_| invalid(format!("unknown time zone {s:?}")))
    }

    /// `YYYY-MM-DD` of the day containing unix timestamp `ts` (seconds) in this zone.
    pub fn day_for_ts(&self, ts: i64) -> String {
        let Ok(instant) = Timestamp::from_second(ts) else {
            // jiff covers years -9999..=9999, so only absurd values land here. Ingest
            // refuses a negative ts or one more than a day ahead, but the 0.1 importer and
            // old rows bypass that check. Bucket them deterministically instead of failing.
            return "1970-01-01".to_string();
        };
        let zone = match self {
            DayZone::Local => TimeZone::system(),
            DayZone::Fixed(secs) => TimeZone::fixed(Offset::from_seconds(*secs).unwrap_or(Offset::UTC)),
            DayZone::Named(tz) => tz.clone(),
        };
        instant.to_zoned(zone).date().to_string()
    }
}

/// Validate a strict `YYYY-MM-DD` calendar date (rejects `2024-02-30`, `2024-2-1`, ...).
pub fn validate_date(value: &str) -> Result<()> {
    // The round trip refuses the other ISO 8601 spellings jiff accepts (`20240201`, `+002024-02-01`).
    match value.parse::<Date>() {
        Ok(d) if d.to_string() == value => Ok(()),
        _ => Err(invalid(format!("date must be a real YYYY-MM-DD calendar date, got {value:?}"))),
    }
}

/// Current unix time in seconds.
pub fn now_ts() -> i64 {
    Timestamp::now().as_second()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_buckets() {
        // 2024-03-10T23:30:00Z
        let ts = 1_710_113_400;
        assert_eq!(DayZone::Fixed(0).day_for_ts(ts), "2024-03-10");
        assert_eq!(DayZone::parse("+01:00").unwrap().day_for_ts(ts), "2024-03-11");
        assert_eq!(DayZone::parse("-0800").unwrap().day_for_ts(ts), "2024-03-10");
        assert_eq!(DayZone::parse("Asia/Kolkata").unwrap().day_for_ts(ts), "2024-03-11");
        assert!(DayZone::parse("Mars/Base").is_err());
        assert!(validate_date("2024-02-30").is_err());
        assert!(validate_date("2024-2-01").is_err());
        assert!(validate_date("2024-02-29").is_ok());
        assert!(validate_date("20240229").is_err());
        assert!(validate_date("+002024-02-29").is_err());
    }

    #[test]
    fn named_zones_follow_dst() {
        // 2024-03-31T00:30:00Z: 01:30 in London (GMT), 02:30 in Berlin (CEST, after the switch).
        let ts = 1_711_845_000;
        let berlin = DayZone::parse("Europe/Berlin").unwrap();
        assert_eq!(berlin.day_for_ts(ts), "2024-03-31");
        // 2024-03-30T23:30:00Z: still 2024-03-31 in Berlin (UTC+1 before the switch).
        assert_eq!(berlin.day_for_ts(ts - 3600), "2024-03-31");
        assert_eq!(DayZone::parse("America/Los_Angeles").unwrap().day_for_ts(ts), "2024-03-30");
        assert_eq!(DayZone::Fixed(0).day_for_ts(i64::MAX), "1970-01-01");
    }
}
