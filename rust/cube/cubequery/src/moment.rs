//! The subset of `moment`/`moment-timezone` semantics the gateway relies on:
//! time-zone aware `add`, `startOf`, `endOf`, the `YYYY-MM-DDTHH:mm:ss.SSS`
//! output format and `moment.utc(string)` parsing of absolute values.

use chrono::{
    DateTime, Datelike, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone,
    Timelike, Utc, Weekday,
};
use chrono_tz::Tz;

/// `moment.HTML5_FMT.DATETIME_LOCAL_MS`.
pub const DATETIME_LOCAL_MS: &str = "%Y-%m-%dT%H:%M:%S%.3f";

/// Calendar / clock units understood by `add`, `startOf` and `endOf`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimeUnit {
    Second,
    Minute,
    Hour,
    Day,
    /// Locale week (Sunday based), what `moment` calls `week`.
    Week,
    /// ISO week (Monday based), what `moment` calls `isoWeek`.
    IsoWeek,
    Month,
    Quarter,
    Year,
}

impl TimeUnit {
    /// Parses the unit names accepted by the date parser regexes
    /// (`day|week|month|year|quarter|hour|minute|second`). `week` maps to the
    /// locale week; the date parser upgrades it to `IsoWeek` for `startOf`.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "second" => TimeUnit::Second,
            "minute" => TimeUnit::Minute,
            "hour" => TimeUnit::Hour,
            "day" => TimeUnit::Day,
            "week" => TimeUnit::Week,
            "month" => TimeUnit::Month,
            "quarter" => TimeUnit::Quarter,
            "year" => TimeUnit::Year,
            _ => return None,
        })
    }
}

/// Truncates a UTC instant to millisecond precision, the precision of a
/// JavaScript `Date`.
pub fn truncate_to_millis(dt: DateTime<Utc>) -> DateTime<Utc> {
    let nanos = dt.nanosecond();
    dt.with_nanosecond(nanos - nanos % 1_000_000).unwrap_or(dt)
}

/// Interprets a wall-clock time in `tz` the way moment-timezone does: for an
/// ambiguous time (DST fall back) the earlier instant is used, for a
/// non-existent time (DST gap) the clock is moved forward past the gap.
pub fn resolve_local(tz: Tz, naive: NaiveDateTime) -> DateTime<Tz> {
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(dt) => dt,
        LocalResult::Ambiguous(earlier, _) => earlier,
        LocalResult::None => {
            let mut shifted = naive;
            for _ in 0..4 {
                shifted += Duration::hours(1);
                if let LocalResult::Single(dt) = tz.from_local_datetime(&shifted) {
                    return dt;
                }
            }
            // Should be unreachable for real time zones; fall back to UTC.
            tz.from_utc_datetime(&naive)
        }
    }
}

/// Adds `months` calendar months, clamping the day of month like moment.
pub fn add_months_clamped(date: NaiveDate, months: i64) -> NaiveDate {
    let total = date.year() as i64 * 12 + (date.month0() as i64) + months;
    let year = total.div_euclid(12);
    let month0 = total.rem_euclid(12);
    let year = year.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
    let month = month0 as u32 + 1;
    let day = date.day().min(days_in_month(year, month));
    NaiveDate::from_ymd_opt(year, month, day)
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(year, month, 1).unwrap_or(date))
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .and_then(|next| next.pred_opt())
        .map(|last| last.day())
        .unwrap_or(28)
}

/// Adds `amount` units to a naive (wall-clock) datetime. Days, weeks, months,
/// quarters and years are calendar arithmetic; hours, minutes and seconds are
/// exact durations.
pub fn naive_add(naive: NaiveDateTime, amount: i64, unit: TimeUnit) -> NaiveDateTime {
    match unit {
        TimeUnit::Second => naive + Duration::seconds(amount),
        TimeUnit::Minute => naive + Duration::minutes(amount),
        TimeUnit::Hour => naive + Duration::hours(amount),
        TimeUnit::Day => naive + Duration::days(amount),
        TimeUnit::Week | TimeUnit::IsoWeek => naive + Duration::days(amount * 7),
        TimeUnit::Month => add_months_clamped(naive.date(), amount).and_time(naive.time()),
        TimeUnit::Quarter => add_months_clamped(naive.date(), amount * 3).and_time(naive.time()),
        TimeUnit::Year => add_months_clamped(naive.date(), amount * 12).and_time(naive.time()),
    }
}

/// `moment.add(amount, unit)` for a time-zone aware moment: calendar units
/// keep the wall-clock time across DST changes, clock units are exact.
pub fn add(dt: DateTime<Tz>, amount: i64, unit: TimeUnit) -> DateTime<Tz> {
    match unit {
        TimeUnit::Second | TimeUnit::Minute | TimeUnit::Hour => {
            let utc = naive_add(dt.naive_utc(), amount, unit);
            dt.timezone().from_utc_datetime(&utc)
        }
        _ => resolve_local(dt.timezone(), naive_add(dt.naive_local(), amount, unit)),
    }
}

fn naive_start_of(naive: NaiveDateTime, unit: TimeUnit) -> NaiveDateTime {
    let date = naive.date();
    let time = naive.time();
    let midnight = NaiveTime::from_hms_opt(0, 0, 0).expect("midnight");
    match unit {
        TimeUnit::Second => date.and_time(
            NaiveTime::from_hms_opt(time.hour(), time.minute(), time.second()).expect("time"),
        ),
        TimeUnit::Minute => {
            date.and_time(NaiveTime::from_hms_opt(time.hour(), time.minute(), 0).expect("time"))
        }
        TimeUnit::Hour => date.and_time(NaiveTime::from_hms_opt(time.hour(), 0, 0).expect("time")),
        TimeUnit::Day => date.and_time(midnight),
        TimeUnit::Week => {
            let back = date.weekday().num_days_from_sunday() as i64;
            (date - Duration::days(back)).and_time(midnight)
        }
        TimeUnit::IsoWeek => {
            let back = date.weekday().num_days_from_monday() as i64;
            (date - Duration::days(back)).and_time(midnight)
        }
        TimeUnit::Month => date.with_day(1).expect("first of month").and_time(midnight),
        TimeUnit::Quarter => {
            let first_month = (date.month0() / 3) * 3 + 1;
            NaiveDate::from_ymd_opt(date.year(), first_month, 1)
                .expect("first of quarter")
                .and_time(midnight)
        }
        TimeUnit::Year => NaiveDate::from_ymd_opt(date.year(), 1, 1)
            .expect("first of year")
            .and_time(midnight),
    }
}

/// `moment.startOf(unit)` on the wall-clock time of `dt`.
pub fn start_of(dt: DateTime<Tz>, unit: TimeUnit) -> DateTime<Tz> {
    resolve_local(dt.timezone(), naive_start_of(dt.naive_local(), unit))
}

/// `moment.endOf(unit)`: the last millisecond of the unit containing `dt`.
pub fn end_of(dt: DateTime<Tz>, unit: TimeUnit) -> DateTime<Tz> {
    let start = naive_start_of(dt.naive_local(), unit);
    let next = naive_add(start, 1, unit);
    let end = next - Duration::milliseconds(1);
    match unit {
        TimeUnit::Second | TimeUnit::Minute | TimeUnit::Hour => {
            // Clock units are exact durations from the (resolved) start.
            let start_dt = resolve_local(dt.timezone(), start);
            let utc = naive_add(start_dt.naive_utc(), 1, unit) - Duration::milliseconds(1);
            dt.timezone().from_utc_datetime(&utc)
        }
        _ => resolve_local(dt.timezone(), end),
    }
}

/// Formats the wall-clock time as `YYYY-MM-DDTHH:mm:ss.SSS`.
pub fn format_local(dt: &DateTime<Tz>) -> String {
    dt.naive_local().format(DATETIME_LOCAL_MS).to_string()
}

/// Formats a naive datetime as `YYYY-MM-DDTHH:mm:ss.SSS`.
pub fn format_naive(naive: &NaiveDateTime) -> String {
    naive.format(DATETIME_LOCAL_MS).to_string()
}

/// Sunday-based weekday index, as JavaScript's `Date#getDay()`.
pub fn js_weekday(weekday: Weekday) -> i64 {
    weekday.num_days_from_sunday() as i64
}

/// `moment.utc(string)` for the ISO 8601 shapes that reach the gateway:
/// `YYYY`, `YYYY-MM`, `YYYY-MM-DD`, optionally followed by `T` or a space and
/// `HH`, `HH:mm`, `HH:mm:ss`, `HH:mm:ss.fraction`, optionally followed by `Z`
/// or a numeric UTC offset (which is applied, so the result is UTC).
pub fn parse_utc(input: &str) -> Option<NaiveDateTime> {
    let s = input.trim();
    let bytes = s.as_bytes();
    let mut pos = 0;

    fn take_digits(bytes: &[u8], pos: &mut usize, min: usize, max: usize) -> Option<i64> {
        let start = *pos;
        while *pos < bytes.len() && bytes[*pos].is_ascii_digit() && *pos - start < max {
            *pos += 1;
        }
        if *pos - start < min {
            return None;
        }
        std::str::from_utf8(&bytes[start..*pos]).ok()?.parse().ok()
    }

    let year = take_digits(bytes, &mut pos, 4, 4)?;
    let mut month = 1;
    let mut day = 1;
    if pos < bytes.len() && bytes[pos] == b'-' {
        pos += 1;
        month = take_digits(bytes, &mut pos, 2, 2)?;
        if pos < bytes.len() && bytes[pos] == b'-' {
            pos += 1;
            day = take_digits(bytes, &mut pos, 2, 2)?;
        }
    }
    let date = NaiveDate::from_ymd_opt(year as i32, month as u32, day as u32)?;

    let (mut hour, mut minute, mut second, mut nanos) = (0, 0, 0, 0u32);
    let mut offset_minutes = 0i64;
    if pos < bytes.len() {
        if bytes[pos] != b'T' && bytes[pos] != b't' && bytes[pos] != b' ' {
            return None;
        }
        pos += 1;
        hour = take_digits(bytes, &mut pos, 2, 2)?;
        if pos < bytes.len() && bytes[pos] == b':' {
            pos += 1;
            minute = take_digits(bytes, &mut pos, 2, 2)?;
            if pos < bytes.len() && bytes[pos] == b':' {
                pos += 1;
                second = take_digits(bytes, &mut pos, 2, 2)?;
                if pos < bytes.len() && (bytes[pos] == b'.' || bytes[pos] == b',') {
                    pos += 1;
                    let start = pos;
                    let fraction = take_digits(bytes, &mut pos, 1, 9)?;
                    let digits = pos - start;
                    nanos = (fraction * 10i64.pow(9 - digits as u32)) as u32;
                }
            }
        }
        if pos < bytes.len() {
            match bytes[pos] {
                b'Z' | b'z' => pos += 1,
                b'+' | b'-' => {
                    let sign = if bytes[pos] == b'-' { -1 } else { 1 };
                    pos += 1;
                    let oh = take_digits(bytes, &mut pos, 2, 2)?;
                    let mut om = 0;
                    if pos < bytes.len() && bytes[pos] == b':' {
                        pos += 1;
                        om = take_digits(bytes, &mut pos, 2, 2)?;
                    } else if pos < bytes.len() && bytes[pos].is_ascii_digit() {
                        om = take_digits(bytes, &mut pos, 2, 2)?;
                    }
                    offset_minutes = sign * (oh * 60 + om);
                }
                _ => return None,
            }
        }
    }
    if pos != bytes.len() {
        return None;
    }

    let time = NaiveTime::from_hms_nano_opt(hour as u32, minute as u32, second as u32, nanos)?;
    Some(date.and_time(time) - Duration::minutes(offset_minutes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tz_dt(tz: &str, s: &str) -> DateTime<Tz> {
        let tz: Tz = tz.parse().unwrap();
        let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.3f").unwrap();
        resolve_local(tz, naive)
    }

    #[test]
    fn start_and_end_of_units() {
        let dt = tz_dt("UTC", "2021-02-15T13:07:08.123");
        assert_eq!(
            format_local(&start_of(dt, TimeUnit::Day)),
            "2021-02-15T00:00:00.000"
        );
        assert_eq!(
            format_local(&end_of(dt, TimeUnit::Day)),
            "2021-02-15T23:59:59.999"
        );
        // 2021-02-15 is a Monday.
        assert_eq!(
            format_local(&start_of(dt, TimeUnit::IsoWeek)),
            "2021-02-15T00:00:00.000"
        );
        assert_eq!(
            format_local(&start_of(dt, TimeUnit::Week)),
            "2021-02-14T00:00:00.000"
        );
        assert_eq!(
            format_local(&end_of(dt, TimeUnit::IsoWeek)),
            "2021-02-21T23:59:59.999"
        );
        assert_eq!(
            format_local(&start_of(dt, TimeUnit::Month)),
            "2021-02-01T00:00:00.000"
        );
        assert_eq!(
            format_local(&end_of(dt, TimeUnit::Month)),
            "2021-02-28T23:59:59.999"
        );
        assert_eq!(
            format_local(&start_of(dt, TimeUnit::Quarter)),
            "2021-01-01T00:00:00.000"
        );
        assert_eq!(
            format_local(&end_of(dt, TimeUnit::Quarter)),
            "2021-03-31T23:59:59.999"
        );
        assert_eq!(
            format_local(&start_of(dt, TimeUnit::Year)),
            "2021-01-01T00:00:00.000"
        );
        assert_eq!(
            format_local(&end_of(dt, TimeUnit::Year)),
            "2021-12-31T23:59:59.999"
        );
        assert_eq!(
            format_local(&start_of(dt, TimeUnit::Hour)),
            "2021-02-15T13:00:00.000"
        );
        assert_eq!(
            format_local(&end_of(dt, TimeUnit::Hour)),
            "2021-02-15T13:59:59.999"
        );
        assert_eq!(
            format_local(&end_of(dt, TimeUnit::Minute)),
            "2021-02-15T13:07:59.999"
        );
        assert_eq!(
            format_local(&end_of(dt, TimeUnit::Second)),
            "2021-02-15T13:07:08.999"
        );
    }

    #[test]
    fn add_clamps_month_days() {
        let dt = tz_dt("UTC", "2021-01-31T10:00:00.000");
        assert_eq!(
            format_local(&add(dt, 1, TimeUnit::Month)),
            "2021-02-28T10:00:00.000"
        );
        assert_eq!(
            format_local(&add(dt, -2, TimeUnit::Month)),
            "2020-11-30T10:00:00.000"
        );
        assert_eq!(
            format_local(&add(dt, 1, TimeUnit::Quarter)),
            "2021-04-30T10:00:00.000"
        );
        let leap = tz_dt("UTC", "2024-02-29T10:00:00.000");
        assert_eq!(
            format_local(&add(leap, 1, TimeUnit::Year)),
            "2025-02-28T10:00:00.000"
        );
    }

    #[test]
    fn add_days_keeps_wall_clock_across_dst() {
        // DST starts 2021-03-14 in Los Angeles.
        let dt = tz_dt("America/Los_Angeles", "2021-03-13T12:00:00.000");
        assert_eq!(
            format_local(&add(dt, 1, TimeUnit::Day)),
            "2021-03-14T12:00:00.000"
        );
        // Hours are exact durations: 23 wall-clock hours later.
        assert_eq!(
            format_local(&add(dt, 24, TimeUnit::Hour)),
            "2021-03-14T13:00:00.000"
        );
    }

    #[test]
    fn parses_moment_utc_shapes() {
        let f = |s: &str| parse_utc(s).map(|d| format_naive(&d));
        assert_eq!(f("2020-01-01").as_deref(), Some("2020-01-01T00:00:00.000"));
        assert_eq!(f("2020-01").as_deref(), Some("2020-01-01T00:00:00.000"));
        assert_eq!(f("2020").as_deref(), Some("2020-01-01T00:00:00.000"));
        assert_eq!(
            f("2020-01-01T10:20").as_deref(),
            Some("2020-01-01T10:20:00.000")
        );
        assert_eq!(
            f("2020-01-01 10:20:30").as_deref(),
            Some("2020-01-01T10:20:30.000")
        );
        assert_eq!(
            f("2020-01-01T10:20:30.5").as_deref(),
            Some("2020-01-01T10:20:30.500")
        );
        assert_eq!(
            f("2020-01-01T10:20:30.123456").as_deref(),
            Some("2020-01-01T10:20:30.123")
        );
        assert_eq!(
            f("2020-01-01T10:20:30.000Z").as_deref(),
            Some("2020-01-01T10:20:30.000")
        );
        assert_eq!(
            f("2024-01-01T00:00:00+02:00").as_deref(),
            Some("2023-12-31T22:00:00.000")
        );
        assert_eq!(
            f("2024-01-31T23:59:59-05:00").as_deref(),
            Some("2024-02-01T04:59:59.000")
        );
        assert_eq!(f("last week"), None);
        assert_eq!(f("2020-13-01"), None);
        assert_eq!(f(""), None);
    }
}
