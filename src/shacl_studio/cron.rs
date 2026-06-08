//! Minimal standard 5-field cron evaluator (UTC) for the pipeline scheduler.
//!
//! Fields: `minute hour day-of-month month day-of-week`. Each field accepts
//! `*`, a single integer, a `a-b` range, a `*/n` or `a-b/n` step, and
//! comma-separated lists of those. Day-of-week is 0–6 with Sunday = 0 (7 also
//! accepted as Sunday). This is intentionally small — the UI's friendly
//! schedule builder emits the cron string; we only need a correct "is this due
//! at minute T" test for a 60-second scheduler tick.

use chrono::{DateTime, Datelike, Timelike, Utc};

/// True when `cron` fires at the given instant (minute resolution). Returns
/// false for a malformed expression rather than erroring — a bad schedule
/// simply never fires.
pub fn is_due(cron: &str, t: DateTime<Utc>) -> bool {
    let fields: Vec<&str> = cron.split_whitespace().collect();
    if fields.len() != 5 {
        return false;
    }
    let minute = t.minute();
    let hour = t.hour();
    let dom = t.day();
    let month = t.month();
    // chrono: Mon=0..Sun=6 via num_days_from_monday; cron wants Sun=0..Sat=6.
    let dow = t.weekday().num_days_from_sunday();

    let min_ok = field_matches(fields[0], minute, 0, 59);
    let hour_ok = field_matches(fields[1], hour, 0, 23);
    let mon_ok = field_matches(fields[3], month, 1, 12);

    // Day-of-month / day-of-week: standard cron uses OR when both are
    // restricted, AND when either is `*`.
    let dom_field = fields[2];
    let dow_field = fields[4];
    let dom_ok = field_matches(dom_field, dom, 1, 31);
    let dow_ok = dow_field_matches(dow_field, dow);
    let day_ok = if dom_field == "*" || dow_field == "*" {
        dom_ok && dow_ok
    } else {
        dom_ok || dow_ok
    };

    min_ok && hour_ok && mon_ok && day_ok
}

fn dow_field_matches(field: &str, dow_sun0: u32) -> bool {
    // Accept 7 as Sunday by normalising the value space to 0..=7.
    field.split(',').any(|part| {
        match_part(part, dow_sun0, 0, 7) || (dow_sun0 == 0 && match_part(part, 7, 0, 7))
    })
}

fn field_matches(field: &str, value: u32, lo: u32, hi: u32) -> bool {
    field.split(',').any(|part| match_part(part, value, lo, hi))
}

fn match_part(part: &str, value: u32, lo: u32, hi: u32) -> bool {
    let part = part.trim();
    // Step: "<range-or-*>/<n>"
    if let Some((base, step_s)) = part.split_once('/') {
        let step: u32 = match step_s.parse() {
            Ok(s) if s > 0 => s,
            _ => return false,
        };
        let (start, end) = range_bounds(base, lo, hi);
        return value >= start && value <= end && (value - start).is_multiple_of(step);
    }
    if part == "*" {
        return value >= lo && value <= hi;
    }
    if let Some((a, b)) = part.split_once('-') {
        if let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>()) {
            return value >= a && value <= b;
        }
        return false;
    }
    part.parse::<u32>().map(|n| n == value).unwrap_or(false)
}

fn range_bounds(base: &str, lo: u32, hi: u32) -> (u32, u32) {
    if base == "*" {
        return (lo, hi);
    }
    if let Some((a, b)) = base.split_once('-') {
        if let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>()) {
            return (a, b);
        }
    }
    if let Ok(n) = base.parse::<u32>() {
        return (n, hi);
    }
    (lo, hi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn every_minute() {
        assert!(is_due("* * * * *", at(2026, 5, 28, 13, 7)));
    }

    #[test]
    fn specific_minute_hour() {
        // 2026-05-28 is a Thursday.
        assert!(is_due("30 9 * * *", at(2026, 5, 28, 9, 30)));
        assert!(!is_due("30 9 * * *", at(2026, 5, 28, 9, 31)));
        assert!(!is_due("30 9 * * *", at(2026, 5, 28, 10, 30)));
    }

    #[test]
    fn step_and_list() {
        assert!(is_due("*/15 * * * *", at(2026, 5, 28, 0, 0)));
        assert!(is_due("*/15 * * * *", at(2026, 5, 28, 0, 45)));
        assert!(!is_due("*/15 * * * *", at(2026, 5, 28, 0, 46)));
        assert!(is_due("0,30 * * * *", at(2026, 5, 28, 4, 30)));
    }

    #[test]
    fn day_of_week_sunday() {
        // 2026-05-31 is a Sunday.
        assert!(is_due("0 0 * * 0", at(2026, 5, 31, 0, 0)));
        assert!(is_due("0 0 * * 7", at(2026, 5, 31, 0, 0)));
        assert!(!is_due("0 0 * * 1", at(2026, 5, 31, 0, 0)));
    }

    #[test]
    fn dom_dow_or_semantics() {
        // Both restricted → OR. Fire on the 1st OR on Mondays.
        // 2026-06-01 is a Monday (matches both); 2026-06-08 is a Monday (dow).
        assert!(is_due("0 0 1 * 1", at(2026, 6, 8, 0, 0)));
        // 2026-06-15 is a Monday → dow matches.
        assert!(is_due("0 0 1 * 1", at(2026, 6, 15, 0, 0)));
        // A non-Monday, non-1st day should not fire.
        assert!(!is_due("0 0 1 * 1", at(2026, 6, 9, 0, 0)));
    }

    #[test]
    fn malformed_never_fires() {
        assert!(!is_due("", at(2026, 5, 28, 0, 0)));
        assert!(!is_due("* * *", at(2026, 5, 28, 0, 0)));
        assert!(!is_due("bogus", at(2026, 5, 28, 0, 0)));
    }
}
