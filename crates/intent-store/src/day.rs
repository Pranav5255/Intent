//! Calendar-day bucketing.
//!
//! 1.x computed an event's day from the *current* process time zone at read time, so
//! changing TZ (or running the engine and capture server under different TZs) silently
//! moved events between days. 2.0 computes the day once, at insert time, from an explicit
//! [`DayZone`] and stores it in `events.day`.

use chrono::{DateTime, FixedOffset, Local, NaiveDate, Utc};
use chrono_tz::Tz;

use crate::error::{Result, invalid};

/// Time zone used to assign events to calendar days.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DayZone {
    /// The host's local zone as reported by the OS when the event is inserted.
    #[default]
    Local,
    /// A fixed UTC offset in seconds east of UTC (no DST).
    Fixed(i32),
    /// A named IANA zone (DST-aware), e.g. `Europe/Berlin`.
    Named(Tz),
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
        s.parse::<Tz>().map(DayZone::Named).map_err(|_| invalid(format!("unknown time zone {s:?}")))
    }

    /// `YYYY-MM-DD` of the day containing unix timestamp `ts` (seconds) in this zone.
    pub fn day_for_ts(&self, ts: i64) -> String {
        let Some(utc) = DateTime::<Utc>::from_timestamp(ts, 0) else {
            // Out-of-range timestamps are rejected upstream; bucket them deterministically.
            return "1970-01-01".to_string();
        };
        let date = match self {
            DayZone::Local => utc.with_timezone(&Local).date_naive(),
            DayZone::Fixed(secs) => {
                let off = FixedOffset::east_opt(*secs).unwrap_or_else(|| FixedOffset::east_opt(0).unwrap());
                utc.with_timezone(&off).date_naive()
            }
            DayZone::Named(tz) => utc.with_timezone(tz).date_naive(),
        };
        date.format("%Y-%m-%d").to_string()
    }
}

/// Validate a strict `YYYY-MM-DD` calendar date (rejects `2024-02-30`, `2024-2-1`, ...).
pub fn validate_date(value: &str) -> Result<()> {
    match NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        Ok(d) if d.format("%Y-%m-%d").to_string() == value => Ok(()),
        _ => Err(invalid(format!("date must be a real YYYY-MM-DD calendar date, got {value:?}"))),
    }
}

/// Current unix time in seconds.
pub fn now_ts() -> i64 {
    Utc::now().timestamp()
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
    }
}
