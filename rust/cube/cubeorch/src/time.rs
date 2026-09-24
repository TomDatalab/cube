//! Time series generation and timezone conversion.
//!
//! Port of the half of `@cubejs-backend/shared`'s `src/time.ts` that the partition range
//! loader needs: [`time_series`], [`time_series_boundaries`], [`local_timestamp_to_utc`],
//! [`utc_to_local_time_zone`], [`parse_utc_into_local_date`] and
//! [`add_seconds_to_local_timestamp`], plus the four parameter placeholders.
//!
//! The Node implementation is built on `moment-timezone` and `moment-range`; this one is
//! built on `chrono` and `chrono-tz`. Two behaviours of the original are reproduced
//! deliberately rather than "fixed", because the partition table names and the version
//! hashes derived from them must not move:
//!
//! * a *local* timestamp is a naked `YYYY-MM-DDTHH:mm:ss.SSS` string with no zone, and all
//!   partition arithmetic is calendar arithmetic on it (`moment` without a zone);
//! * the zone offset used to convert one of those strings is looked up **at the instant the
//!   string denotes when read as UTC**, not at the instant it denotes in its own zone
//!   (`zone.utcOffset(Date.parse(`${timestamp}Z`))`). The two differ only inside a DST
//!   transition, and only there does this port differ from a "correct" conversion.

use chrono::{
    DateTime, Datelike, Days, LocalResult, Months, NaiveDate, NaiveDateTime, Offset, TimeZone,
    Timelike, Utc,
};
use chrono_tz::Tz;
use serde_json::Value;

use crate::error::OrchError;

/// `FROM_PARTITION_RANGE` — the placeholder a partition's lower bound replaces.
pub const FROM_PARTITION_RANGE: &str = "__FROM_PARTITION_RANGE";
/// `TO_PARTITION_RANGE` — the placeholder a partition's upper bound replaces.
pub const TO_PARTITION_RANGE: &str = "__TO_PARTITION_RANGE";
/// `BUILD_RANGE_START_LOCAL` — replaced in the *user* query's values, not in the build SQL.
pub const BUILD_RANGE_START_LOCAL: &str = "__BUILD_RANGE_START_LOCAL";
/// `BUILD_RANGE_END_LOCAL`.
pub const BUILD_RANGE_END_LOCAL: &str = "__BUILD_RANGE_END_LOCAL";

/// `DEFAULT_TS_FORMAT` of `PreAggregationPartitionRangeLoader`.
pub const DEFAULT_TS_FORMAT: &str = "YYYY-MM-DDTHH:mm:ss.SSS";

/// The soft limit `checkSeriesForDateRange` enforces before a series is materialized.
const DATE_RANGE_COUNT_LIMIT: f64 = 50000.0;

/// `QueryDateRange = [string, string]` — a **local** timestamp pair.
pub type QueryDateRange = (String, String);

/// The predefined partition granularities (`TIME_SERIES` of `time.ts:96-116`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Granularity {
    Second,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

impl Granularity {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "second" => Granularity::Second,
            "minute" => Granularity::Minute,
            "hour" => Granularity::Hour,
            "day" => Granularity::Day,
            "week" => Granularity::Week,
            "month" => Granularity::Month,
            "quarter" => Granularity::Quarter,
            "year" => Granularity::Year,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Granularity::Second => "second",
            Granularity::Minute => "minute",
            Granularity::Hour => "hour",
            Granularity::Day => "day",
            Granularity::Week => "week",
            Granularity::Month => "month",
            Granularity::Quarter => "quarter",
            Granularity::Year => "year",
        }
    }

    /// `moment.duration(1, unit).asSeconds()`: a month is 146097/4800 days, which is what
    /// `checkSeriesForDateRange` divides the requested span by.
    fn duration_seconds(self) -> f64 {
        match self {
            Granularity::Second => 1.0,
            Granularity::Minute => 60.0,
            Granularity::Hour => 3600.0,
            Granularity::Day => 86400.0,
            Granularity::Week => 604800.0,
            Granularity::Month => 2629746.0,
            Granularity::Quarter => 3.0 * 2629746.0,
            Granularity::Year => 12.0 * 2629746.0,
        }
    }

    /// `range.snapTo(unit).start` — the beginning of the unit `dt` falls in.
    fn snap_start(self, dt: NaiveDateTime) -> NaiveDateTime {
        let date = dt.date();

        match self {
            Granularity::Second => dt.with_nanosecond(0).unwrap_or(dt),
            Granularity::Minute => date.and_hms_opt(dt.hour(), dt.minute(), 0).unwrap_or(dt),
            Granularity::Hour => date.and_hms_opt(dt.hour(), 0, 0).unwrap_or(dt),
            Granularity::Day => date.and_hms_opt(0, 0, 0).unwrap_or(dt),
            // `startOf('isoWeek')` — Monday.
            Granularity::Week => {
                let back = date.weekday().num_days_from_monday() as u64;
                (date - Days::new(back)).and_hms_opt(0, 0, 0).unwrap_or(dt)
            }
            Granularity::Month => NaiveDate::from_ymd_opt(date.year(), date.month(), 1)
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .unwrap_or(dt),
            Granularity::Quarter => {
                let month = (date.month() - 1) / 3 * 3 + 1;
                NaiveDate::from_ymd_opt(date.year(), month, 1)
                    .and_then(|d| d.and_hms_opt(0, 0, 0))
                    .unwrap_or(dt)
            }
            Granularity::Year => NaiveDate::from_ymd_opt(date.year(), 1, 1)
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .unwrap_or(dt),
        }
    }

    /// The start of the next unit.
    fn next(self, start: NaiveDateTime) -> NaiveDateTime {
        match self {
            Granularity::Second => start + chrono::Duration::seconds(1),
            Granularity::Minute => start + chrono::Duration::minutes(1),
            Granularity::Hour => start + chrono::Duration::hours(1),
            Granularity::Day => start + Days::new(1),
            Granularity::Week => start + Days::new(7),
            Granularity::Month => start + Months::new(1),
            Granularity::Quarter => start + Months::new(3),
            Granularity::Year => start + Months::new(12),
        }
    }

    /// `endOf(unit)` of an already snapped `start`, to the second — the sub-second part is
    /// written by the formatter as `timestampPrecision` nines.
    fn end_of(self, start: NaiveDateTime) -> NaiveDateTime {
        self.next(start) - chrono::Duration::seconds(1)
    }
}

fn zeros(digits: u32) -> String {
    "0".repeat(digits as usize)
}

fn nines(digits: u32) -> String {
    "9".repeat(digits as usize)
}

fn format_range(granularity: Granularity, start: NaiveDateTime, digits: u32) -> QueryDateRange {
    let end = granularity.end_of(start);

    (
        format!("{}.{}", start.format("%Y-%m-%dT%H:%M:%S"), zeros(digits)),
        format!("{}.{}", end.format("%Y-%m-%dT%H:%M:%S"), nines(digits)),
    )
}

/// Parses a local timestamp. `moment` accepts a good deal more than this, but every string
/// that reaches the partition loader was produced by [`time_series`] or by
/// [`parse_utc_into_local_date`], both of which write `YYYY-MM-DDTHH:mm:ss.SSS[SSS]`.
pub fn parse_local_timestamp(value: &str) -> Option<NaiveDateTime> {
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
    ] {
        if let Ok(parsed) = NaiveDateTime::parse_from_str(value, format) {
            return Some(parsed);
        }
    }

    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
}

fn granularity_of(value: &str) -> Result<Granularity, OrchError> {
    Granularity::parse(value)
        .ok_or_else(|| OrchError::orchestration(format!("Unsupported time granularity: {value}")))
}

/// `checkTimeSeries` + `checkSeriesForDateRange` (`time.ts:214-263`).
fn check_time_series(
    granularity: Granularity,
    date_range: &QueryDateRange,
    timestamp_precision: u32,
) -> Result<(), OrchError> {
    if timestamp_precision == 0 {
        return Err(OrchError::orchestration(
            "options.timestampPrecision is required, actual: 0",
        ));
    }

    let (Some(start), Some(end)) = (
        parse_local_timestamp(&date_range.0),
        parse_local_timestamp(&date_range.1),
    ) else {
        // `moment` yields an invalid date here and every comparison against it is false, so
        // the limit never trips.
        return Ok(());
    };

    let range_seconds = (end - start).num_seconds() as f64;
    let count = range_seconds / granularity.duration_seconds();

    if count > DATE_RANGE_COUNT_LIMIT {
        return Err(OrchError::orchestration(format!(
            "The count of generated date ranges ({}) for the request from [{}] to [{}] by 1 {} is \
             over limit ({}). Please reduce the requested date interval or use bigger granularity.",
            format_number(count),
            date_range.0,
            date_range.1,
            granularity.as_str(),
            DATE_RANGE_COUNT_LIMIT as u64,
        )));
    }

    Ok(())
}

/// `String(n)` for the numbers that reach the limit message.
fn format_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// `timeSeries(granularity, dateRange, { timestampPrecision })` (`time.ts:274-281`).
///
/// The partitions of `dateRange`, aligned to the granularity: `dateRange` is snapped
/// outwards to whole units and every unit in between is emitted as a `[start, end]` pair
/// whose sub-second part is `timestampPrecision` zeros and nines.
pub fn time_series(
    granularity: &str,
    date_range: &QueryDateRange,
    timestamp_precision: u32,
) -> Result<Vec<QueryDateRange>, OrchError> {
    let granularity = granularity_of(granularity)?;
    check_time_series(granularity, date_range, timestamp_precision)?;

    let (Some(start), Some(end)) = (
        parse_local_timestamp(&date_range.0),
        parse_local_timestamp(&date_range.1),
    ) else {
        return Ok(Vec::new());
    };

    let last = granularity.end_of(granularity.snap_start(end));
    let mut current = granularity.snap_start(start);
    let mut ranges = Vec::new();

    while current <= last {
        ranges.push(format_range(granularity, current, timestamp_precision));
        current = granularity.next(current);
    }

    Ok(ranges)
}

/// `timeSeriesBoundaries` (`time.ts:287-301`) — the first and last partition of
/// [`time_series`] without materializing the ones in between.
pub fn time_series_boundaries(
    granularity: &str,
    date_range: &QueryDateRange,
    timestamp_precision: u32,
) -> Result<(Option<QueryDateRange>, Option<QueryDateRange>), OrchError> {
    let parsed = granularity_of(granularity)?;
    check_time_series(parsed, date_range, timestamp_precision)?;

    let (Some(start), Some(end)) = (
        parse_local_timestamp(&date_range.0),
        parse_local_timestamp(&date_range.1),
    ) else {
        let series = time_series(granularity, date_range, timestamp_precision)?;
        return Ok((series.first().cloned(), series.last().cloned()));
    };

    if start > end {
        let series = time_series(granularity, date_range, timestamp_precision)?;
        return Ok((series.first().cloned(), series.last().cloned()));
    }

    Ok((
        Some(format_range(
            parsed,
            parsed.snap_start(start),
            timestamp_precision,
        )),
        Some(format_range(
            parsed,
            parsed.snap_start(end),
            timestamp_precision,
        )),
    ))
}

// ----------------------------------------------------------------------------
// Timezone conversions
// ----------------------------------------------------------------------------

fn zone(timezone: &str) -> Result<Tz, OrchError> {
    timezone
        .parse::<Tz>()
        .map_err(|_| OrchError::orchestration(format!("Unknown timezone: {timezone}")))
}

/// `zone.utcOffset(ts)` of `moment-timezone`, in seconds and with the opposite sign: the
/// number of seconds the local clock is **ahead** of UTC at the given instant.
fn offset_seconds_at(tz: Tz, instant_as_utc: NaiveDateTime) -> i64 {
    tz.offset_from_utc_datetime(&instant_as_utc)
        .fix()
        .local_minus_utc() as i64
}

/// The four timestamp formats `localTimestampToUtc` and `utcToLocalTimeZone` special case.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KnownTsFormat {
    /// `YYYY-MM-DDTHH:mm:ss.SSS`
    LocalMillis,
    /// `YYYY-MM-DDTHH:mm:ss.SSSSSS`
    LocalMicros,
    /// `YYYY-MM-DDTHH:mm:ss.SSSZ`
    UtcMillis,
    /// `YYYY-MM-DDTHH:mm:ss.SSSSSSZ`
    UtcMicros,
}

fn known_format(format: Option<&str>) -> Option<KnownTsFormat> {
    Some(match format.unwrap_or(DEFAULT_TS_FORMAT) {
        "YYYY-MM-DD[T]HH:mm:ss.SSS[Z]" | "YYYY-MM-DDTHH:mm:ss.SSSZ" => KnownTsFormat::UtcMillis,
        "YYYY-MM-DD[T]HH:mm:ss.SSSSSS[Z]" | "YYYY-MM-DDTHH:mm:ss.SSSSSSZ" => {
            KnownTsFormat::UtcMicros
        }
        "YYYY-MM-DDTHH:mm:ss.SSS" => KnownTsFormat::LocalMillis,
        "YYYY-MM-DDTHH:mm:ss.SSSSSS" => KnownTsFormat::LocalMicros,
        _ => return None,
    })
}

/// `new Date(...).toJSON()`.
fn to_json(instant: DateTime<Utc>) -> String {
    instant.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// The microsecond emulation of `localTimestampToUtc`: `.999Z` becomes `.999999Z`, anything
/// else gains three zeros, because the underlying `Date` only carries milliseconds.
fn to_json_micros(instant: DateTime<Utc>, keep_z: bool) -> String {
    let value = to_json(instant);
    let tail = if value.ends_with("999Z") {
        "999"
    } else {
        "000"
    };

    if keep_z {
        format!("{}{tail}Z", value.trim_end_matches('Z'))
    } else {
        format!("{}{tail}", value.trim_end_matches('Z'))
    }
}

fn render(instant: DateTime<Utc>, format: KnownTsFormat) -> String {
    match format {
        KnownTsFormat::UtcMillis => to_json(instant),
        KnownTsFormat::UtcMicros => to_json_micros(instant, true),
        KnownTsFormat::LocalMillis => to_json(instant).trim_end_matches('Z').to_string(),
        KnownTsFormat::LocalMicros => to_json_micros(instant, false),
    }
}

/// Truncates to milliseconds, the way `Date.parse` does for a six digit fraction.
fn truncate_to_millis(dt: NaiveDateTime) -> NaiveDateTime {
    let nanos = dt.nanosecond();
    dt.with_nanosecond(nanos / 1_000_000 * 1_000_000)
        .unwrap_or(dt)
}

/// `localTimestampToUtc(timezone, timestampFormat, timestamp)` (`time.ts:316-361`), which
/// `PreAggregationPartitionRangeLoader.inDbTimeZone` is a thin wrapper over: reads
/// `timestamp` as a wall clock in `timezone` and renders the UTC instant it denotes.
pub fn local_timestamp_to_utc(
    timezone: &str,
    timestamp_format: Option<&str>,
    timestamp: Option<&str>,
) -> Result<Option<String>, OrchError> {
    let Some(timestamp) = timestamp.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let tz = zone(timezone)?;
    let length = timestamp.chars().count();

    if length == 23 || length == 26 {
        if let (Some(naive), Some(format)) = (
            parse_local_timestamp(timestamp).map(truncate_to_millis),
            known_format(timestamp_format),
        ) {
            let offset = offset_seconds_at(tz, naive);
            let instant = Utc.from_utc_datetime(&naive) - chrono::Duration::seconds(offset);

            return Ok(Some(render(instant, format)));
        }
    }

    // `moment.tz(timestamp, timezone).utc().format(timestampFormat)`.
    let Some(naive) = parse_local_timestamp(timestamp) else {
        return Ok(None);
    };
    let instant = local_to_instant(tz, naive)?;

    Ok(Some(format_moment(
        instant.naive_utc(),
        timestamp_format.unwrap_or(DEFAULT_TS_FORMAT),
    )))
}

/// `utcToLocalTimeZone(timezone, timestampFormat, timestamp)` (`time.ts:363-384`).
pub fn utc_to_local_time_zone(
    timezone: &str,
    timestamp_format: Option<&str>,
    timestamp: Option<&str>,
) -> Result<Option<String>, OrchError> {
    let Some(timestamp) = timestamp.filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let tz = zone(timezone)?;

    if timestamp.chars().count() == 23 {
        if let (Some(naive), Some(format)) = (
            parse_local_timestamp(timestamp).map(truncate_to_millis),
            known_format(timestamp_format).filter(|format| {
                matches!(
                    format,
                    KnownTsFormat::UtcMillis | KnownTsFormat::LocalMillis
                )
            }),
        ) {
            let offset = offset_seconds_at(tz, naive);
            let local = Utc.from_utc_datetime(&naive) + chrono::Duration::seconds(offset);

            return Ok(Some(render(local, format)));
        }
    }

    let Some(naive) = parse_local_timestamp(timestamp) else {
        return Ok(None);
    };
    let local = tz.from_utc_datetime(&naive).naive_local();

    Ok(Some(format_moment(
        local,
        timestamp_format.unwrap_or(DEFAULT_TS_FORMAT),
    )))
}

/// `moment.tz(naive, tz)` — an ambiguous wall clock resolves to the earlier offset, a
/// non-existent one to the instant the transition lands on.
fn local_to_instant(tz: Tz, naive: NaiveDateTime) -> Result<DateTime<Utc>, OrchError> {
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(value) => Ok(value.with_timezone(&Utc)),
        LocalResult::Ambiguous(earlier, _) => Ok(earlier.with_timezone(&Utc)),
        LocalResult::None => {
            // A wall clock that the DST jump skipped: fall back to the standard offset.
            let offset = offset_seconds_at(tz, naive);
            Ok(Utc.from_utc_datetime(&naive) - chrono::Duration::seconds(offset))
        }
    }
}

/// `addSecondsToLocalTimestamp(timestamp, timezone, seconds)` (`time.ts:428-443`) — the
/// instant `seconds` after the wall clock `timestamp` in `timezone`.
pub fn add_seconds_to_local_timestamp(
    timestamp: &str,
    timezone: &str,
    seconds: i64,
) -> Result<DateTime<Utc>, OrchError> {
    let tz = zone(timezone)?;
    let Some(naive) = parse_local_timestamp(timestamp) else {
        return Err(OrchError::orchestration(format!(
            "Timestamp expected to be in {DEFAULT_TS_FORMAT} format but {timestamp} found"
        )));
    };

    let instant = if timestamp.chars().count() == 23 {
        let naive = truncate_to_millis(naive);
        let offset = offset_seconds_at(tz, naive);

        Utc.from_utc_datetime(&naive) - chrono::Duration::seconds(offset)
    } else {
        local_to_instant(tz, naive)?
    };

    Ok(instant + chrono::Duration::seconds(seconds))
}

/// `parseUtcIntoLocalDate(data, timezone, timestampFormat)` (`time.ts:386-426`), which
/// `PreAggregationPartitionRangeLoader.extractDate` is a thin wrapper over.
///
/// `data` is the raw result of a build range query: the first column of its first row is
/// read, interpreted as UTC unless it carries zone information, and rendered as a wall clock
/// in `timezone`.
pub fn parse_utc_into_local_date(
    data: &Value,
    timezone: &str,
    timestamp_format: Option<&str>,
) -> Result<Option<String>, OrchError> {
    let Some(row) = data.as_array().and_then(|rows| rows.first()) else {
        return Ok(None);
    };
    let Some(value) = row
        .as_object()
        .and_then(|row| row.values().next())
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let tz = zone(timezone)?;

    let Some(instant) = parse_timestamp_with_optional_zone(value) else {
        return Ok(None);
    };

    Ok(Some(format_moment(
        tz.from_utc_datetime(&instant.naive_utc()).naive_local(),
        timestamp_format.unwrap_or(DEFAULT_TS_FORMAT),
    )))
}

fn has_zone_suffix(value: &str) -> bool {
    let trimmed = value.trim();

    if trimmed.contains('Z') {
        return true;
    }

    // `([+-]\d{2}:?\d{2})$`
    let bytes = trimmed.as_bytes();
    for length in [5usize, 6] {
        if bytes.len() > length {
            let tail = &trimmed[trimmed.len() - length..];
            let sign = tail.starts_with('+') || tail.starts_with('-');
            let digits = tail[1..].chars().all(|c| c.is_ascii_digit() || c == ':');

            if sign && digits && tail[1..].chars().filter(char::is_ascii_digit).count() == 4 {
                return true;
            }
        }
    }

    false
}

fn parse_timestamp_with_optional_zone(value: &str) -> Option<DateTime<Utc>> {
    let trimmed = value.trim();

    if has_zone_suffix(trimmed) {
        if let Ok(parsed) = DateTime::parse_from_rfc3339(trimmed) {
            return Some(parsed.with_timezone(&Utc));
        }
        for format in ["%Y-%m-%dT%H:%M:%S%.f%#z", "%Y-%m-%d %H:%M:%S%.f%#z"] {
            if let Ok(parsed) = DateTime::parse_from_str(trimmed, format) {
                return Some(parsed.with_timezone(&Utc));
            }
        }
        // `2020-01-01T00:00:00Z` without a fraction is not RFC 3339 for chrono's parser only
        // when the seconds are missing; strip the `Z` and read it as UTC.
        let bare = trimmed.trim_end_matches('Z');

        return parse_local_timestamp(bare).map(|naive| Utc.from_utc_datetime(&naive));
    }

    parse_local_timestamp(trimmed).map(|naive| Utc.from_utc_datetime(&naive))
}

/// `new Date().toJSON()` truncated to the 23 characters `utcToLocalTimeZone` expects, then
/// shifted into `timezone` — `PreAggregationPartitionRangeLoader.now()`.
pub fn now_in_time_zone(timezone: &str) -> Result<String, OrchError> {
    let now = Utc::now().format("%Y-%m-%dT%H:%M:%S%.3f").to_string();

    Ok(utc_to_local_time_zone(timezone, Some(DEFAULT_TS_FORMAT), Some(&now))?.unwrap_or(now))
}

/// The subset of `moment`'s formatting tokens that appears in a `timestampFormat`.
fn format_moment(dt: NaiveDateTime, format: &str) -> String {
    let mut out = String::with_capacity(format.len() + 8);
    let bytes: Vec<char> = format.chars().collect();
    let mut index = 0;

    while index < bytes.len() {
        // `[literal]`
        if bytes[index] == '[' {
            if let Some(end) = bytes[index + 1..].iter().position(|c| *c == ']') {
                out.extend(&bytes[index + 1..index + 1 + end]);
                index += end + 2;
                continue;
            }
        }

        let rest: String = bytes[index..].iter().collect();
        let token = ["SSSSSS", "YYYY", "SSS", "MM", "DD", "HH", "mm", "ss", "YY"]
            .into_iter()
            .find(|token| rest.starts_with(token));

        match token {
            Some("YYYY") => out.push_str(&format!("{:04}", dt.year())),
            Some("YY") => out.push_str(&format!("{:02}", dt.year() % 100)),
            Some("MM") => out.push_str(&format!("{:02}", dt.month())),
            Some("DD") => out.push_str(&format!("{:02}", dt.day())),
            Some("HH") => out.push_str(&format!("{:02}", dt.hour())),
            Some("mm") => out.push_str(&format!("{:02}", dt.minute())),
            Some("ss") => out.push_str(&format!("{:02}", dt.second())),
            Some("SSS") => out.push_str(&format!("{:03}", dt.nanosecond() / 1_000_000)),
            Some("SSSSSS") => out.push_str(&format!("{:06}", dt.nanosecond() / 1_000)),
            _ => {
                out.push(bytes[index]);
                index += 1;
                continue;
            }
        }

        index += token.expect("matched above").len();
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn range(start: &str, end: &str) -> QueryDateRange {
        (start.to_string(), end.to_string())
    }

    /// Ported from `packages/cubejs-backend-shared/test/time.test.ts` and from running the
    /// repository's own `timeSeries` under Node as an oracle.
    #[test]
    fn golden_time_series_day() {
        assert_eq!(
            time_series(
                "day",
                &range("2021-01-01T00:00:00.000", "2021-01-03T23:59:59.999"),
                3
            )
            .unwrap(),
            vec![
                range("2021-01-01T00:00:00.000", "2021-01-01T23:59:59.999"),
                range("2021-01-02T00:00:00.000", "2021-01-02T23:59:59.999"),
                range("2021-01-03T00:00:00.000", "2021-01-03T23:59:59.999"),
            ]
        );
    }

    #[test]
    fn time_series_snaps_a_partial_range_outwards() {
        assert_eq!(
            time_series(
                "day",
                &range("2021-01-01T09:15:00.000", "2021-01-02T10:00:00.000"),
                3
            )
            .unwrap(),
            vec![
                range("2021-01-01T00:00:00.000", "2021-01-01T23:59:59.999"),
                range("2021-01-02T00:00:00.000", "2021-01-02T23:59:59.999"),
            ]
        );
    }

    #[test]
    fn golden_time_series_month_year_hour_and_week() {
        assert_eq!(
            time_series(
                "month",
                &range("2021-01-01T00:00:00.000", "2021-03-31T23:59:59.999"),
                3
            )
            .unwrap(),
            vec![
                range("2021-01-01T00:00:00.000", "2021-01-31T23:59:59.999"),
                range("2021-02-01T00:00:00.000", "2021-02-28T23:59:59.999"),
                range("2021-03-01T00:00:00.000", "2021-03-31T23:59:59.999"),
            ]
        );

        assert_eq!(
            time_series(
                "year",
                &range("2020-05-05T00:00:00.000", "2021-01-01T00:00:00.000"),
                3
            )
            .unwrap(),
            vec![
                range("2020-01-01T00:00:00.000", "2020-12-31T23:59:59.999"),
                range("2021-01-01T00:00:00.000", "2021-12-31T23:59:59.999"),
            ]
        );

        assert_eq!(
            time_series(
                "hour",
                &range("2021-01-01T22:30:00.000", "2021-01-02T00:10:00.000"),
                3
            )
            .unwrap(),
            vec![
                range("2021-01-01T22:00:00.000", "2021-01-01T22:59:59.999"),
                range("2021-01-01T23:00:00.000", "2021-01-01T23:59:59.999"),
                range("2021-01-02T00:00:00.000", "2021-01-02T00:59:59.999"),
            ]
        );

        // `snapTo('isoWeek')`: 2021-01-01 is a Friday, so the first week starts on Monday
        // the 28th of December.
        assert_eq!(
            time_series(
                "week",
                &range("2021-01-01T00:00:00.000", "2021-01-05T00:00:00.000"),
                3
            )
            .unwrap(),
            vec![
                range("2020-12-28T00:00:00.000", "2021-01-03T23:59:59.999"),
                range("2021-01-04T00:00:00.000", "2021-01-10T23:59:59.999"),
            ]
        );

        assert_eq!(
            time_series(
                "quarter",
                &range("2021-02-01T00:00:00.000", "2021-04-02T00:00:00.000"),
                3
            )
            .unwrap(),
            vec![
                range("2021-01-01T00:00:00.000", "2021-03-31T23:59:59.999"),
                range("2021-04-01T00:00:00.000", "2021-06-30T23:59:59.999"),
            ]
        );
    }

    #[test]
    fn timestamp_precision_widens_the_fraction() {
        assert_eq!(
            time_series(
                "day",
                &range("2021-01-01T00:00:00.000000", "2021-01-01T00:00:00.000000"),
                6
            )
            .unwrap(),
            vec![range(
                "2021-01-01T00:00:00.000000",
                "2021-01-01T23:59:59.999999"
            )]
        );
    }

    #[test]
    fn unsupported_granularity_and_precision_are_refused() {
        assert_eq!(
            time_series("fortnight", &range("2021-01-01T00:00:00.000", "x"), 3)
                .unwrap_err()
                .to_string(),
            "Unsupported time granularity: fortnight"
        );
        assert!(
            time_series("day", &range("2021-01-01T00:00:00.000", "x"), 0)
                .unwrap_err()
                .to_string()
                .contains("timestampPrecision is required")
        );
    }

    #[test]
    fn an_over_long_series_is_refused_before_it_is_materialized() {
        let error = time_series(
            "second",
            &range("2021-01-01T00:00:00.000", "2021-01-03T00:00:00.000"),
            3,
        )
        .unwrap_err()
        .to_string();

        assert!(error.starts_with("The count of generated date ranges (172800)"));
        assert!(error.ends_with("is over limit (50000). Please reduce the requested date interval or use bigger granularity."));
    }

    #[test]
    fn boundaries_are_the_first_and_last_partition() {
        let (first, last) = time_series_boundaries(
            "day",
            &range("2021-01-01T09:00:00.000", "2021-06-30T10:00:00.000"),
            3,
        )
        .unwrap();

        assert_eq!(
            first,
            Some(range("2021-01-01T00:00:00.000", "2021-01-01T23:59:59.999"))
        );
        assert_eq!(
            last,
            Some(range("2021-06-30T00:00:00.000", "2021-06-30T23:59:59.999"))
        );

        // They agree with the series they summarise.
        let series = time_series(
            "month",
            &range("2021-01-05T00:00:00.000", "2021-04-05T00:00:00.000"),
            3,
        )
        .unwrap();
        let (first, last) = time_series_boundaries(
            "month",
            &range("2021-01-05T00:00:00.000", "2021-04-05T00:00:00.000"),
            3,
        )
        .unwrap();
        assert_eq!(first.as_ref(), series.first());
        assert_eq!(last.as_ref(), series.last());
    }

    #[test]
    fn local_timestamps_convert_into_utc() {
        assert_eq!(
            local_timestamp_to_utc("UTC", None, Some("2021-01-01T00:00:00.000")).unwrap(),
            Some("2021-01-01T00:00:00.000".to_string())
        );
        // Standard time in New York is UTC-5.
        assert_eq!(
            local_timestamp_to_utc("America/New_York", None, Some("2021-01-01T00:00:00.000"))
                .unwrap(),
            Some("2021-01-01T05:00:00.000".to_string())
        );
        // Daylight saving time is UTC-4.
        assert_eq!(
            local_timestamp_to_utc("America/New_York", None, Some("2021-07-01T00:00:00.000"))
                .unwrap(),
            Some("2021-07-01T04:00:00.000".to_string())
        );
        assert_eq!(
            local_timestamp_to_utc(
                "America/New_York",
                Some("YYYY-MM-DDTHH:mm:ss.SSSZ"),
                Some("2021-01-01T00:00:00.000")
            )
            .unwrap(),
            Some("2021-01-01T05:00:00.000Z".to_string())
        );
        // A partition's upper bound keeps its nines when microseconds are emulated.
        assert_eq!(
            local_timestamp_to_utc(
                "UTC",
                Some("YYYY-MM-DDTHH:mm:ss.SSSSSS"),
                Some("2021-01-01T23:59:59.999")
            )
            .unwrap(),
            Some("2021-01-01T23:59:59.999999".to_string())
        );
        assert_eq!(local_timestamp_to_utc("UTC", None, None).unwrap(), None);
        assert!(local_timestamp_to_utc("Mars/Olympus", None, Some("x"))
            .unwrap_err()
            .to_string()
            .contains("Unknown timezone"));
    }

    #[test]
    fn utc_timestamps_convert_into_a_local_wall_clock() {
        assert_eq!(
            utc_to_local_time_zone("America/New_York", None, Some("2021-01-01T05:00:00.000"))
                .unwrap(),
            Some("2021-01-01T00:00:00.000".to_string())
        );
        assert!(now_in_time_zone("UTC").unwrap().len() == 23);
    }

    #[test]
    fn seconds_are_added_to_a_local_timestamp_in_its_zone() {
        let instant =
            add_seconds_to_local_timestamp("2021-01-01T00:00:00.000", "America/New_York", 3600)
                .unwrap();

        assert_eq!(to_json(instant), "2021-01-01T06:00:00.000Z");

        let instant = add_seconds_to_local_timestamp("2021-01-01T23:59:59.999", "UTC", 0).unwrap();
        assert_eq!(to_json(instant), "2021-01-01T23:59:59.999Z");
    }

    #[test]
    fn a_range_query_row_is_read_into_a_local_date() {
        assert_eq!(
            parse_utc_into_local_date(&json!([{ "max": "2021-01-01T05:00:00.000" }]), "UTC", None)
                .unwrap(),
            Some("2021-01-01T05:00:00.000".to_string())
        );
        assert_eq!(
            parse_utc_into_local_date(
                &json!([{ "max": "2021-01-01 05:00:00" }]),
                "America/New_York",
                None
            )
            .unwrap(),
            Some("2021-01-01T00:00:00.000".to_string())
        );
        // A value that already carries a zone is respected rather than re-interpreted.
        assert_eq!(
            parse_utc_into_local_date(
                &json!([{ "max": "2021-01-01T05:00:00.000Z" }]),
                "America/New_York",
                None
            )
            .unwrap(),
            Some("2021-01-01T00:00:00.000".to_string())
        );
        assert_eq!(
            parse_utc_into_local_date(&json!([{ "max": Value::Null }]), "UTC", None).unwrap(),
            None
        );
        assert_eq!(
            parse_utc_into_local_date(&json!([]), "UTC", None).unwrap(),
            None
        );
    }

    #[test]
    fn moment_format_tokens() {
        let dt = parse_local_timestamp("2021-02-03T04:05:06.789").unwrap();

        assert_eq!(
            format_moment(dt, "YYYY-MM-DDTHH:mm:ss.SSS"),
            "2021-02-03T04:05:06.789"
        );
        assert_eq!(
            format_moment(dt, "YYYY-MM-DD[T]HH:mm:ss.SSS[Z]"),
            "2021-02-03T04:05:06.789Z"
        );
        assert_eq!(format_moment(dt, "YYYY-MM-DD"), "2021-02-03");
    }
}
