//! Port of `packages/cubejs-api-gateway/src/date-parser.js`.
//!
//! Resolves relative date range strings (`last 7 days`, `this month`,
//! `from 7 days ago to now`, `today`, `yesterday`, ...) to an absolute
//! `[start, end]` pair formatted as `YYYY-MM-DDTHH:mm:ss.SSS` wall-clock
//! values in the requested time zone.

use std::sync::LazyLock;

use chrono::{DateTime, NaiveDateTime, Utc};
use chrono_tz::Tz;
use regex::Regex;

use crate::chrono_text;
use crate::error::QueryError;
use crate::moment::{self, TimeUnit};
use crate::timezone::find_timezone;

static THIS_LAST_NEXT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("(this|last|next)\\s+(day|week|month|year|quarter|hour|minute|second)")
        .expect("regex")
});
static LAST_NEXT_N_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("(last|next)\\s+(\\d+)\\s+(day|week|month|year|quarter|hour|minute|second)")
        .expect("regex")
});
static FROM_TO_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("^from (.*) to (.*)$").expect("regex"));
static FROM_TO_BOUNDED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("^from(.{0,50})to(.{0,50})$").expect("regex"));

/// A resolved date range: `[start, end]` as `YYYY-MM-DDTHH:mm:ss.SSS`.
pub type DateRangeBounds = [String; 2];

/// `dateParser(dateString, timezone)` with the current time as `now`.
pub fn date_parser(date_string: &str, timezone: &str) -> Result<DateRangeBounds, QueryError> {
    date_parser_with_now(date_string, timezone, Utc::now())
}

fn cannot_parse(text: &str) -> QueryError {
    QueryError::user(format!("Can't parse date: '{text}'"))
}

/// `dateParser(dateString, timezone, now)`.
///
/// `timezone` must be an IANA name (matched case-insensitively).
pub fn date_parser_with_now(
    date_string: &str,
    timezone: &str,
    now: DateTime<Utc>,
) -> Result<DateRangeBounds, QueryError> {
    let tz = find_timezone(timezone)
        .ok_or_else(|| QueryError::user(format!("Unknown timezone: '{timezone}'")))?;
    let now = moment::truncate_to_millis(now).with_timezone(&tz);
    let date_string = date_string.to_lowercase();

    let range: [DateTime<Tz>; 2] = if let Some(m) = THIS_LAST_NEXT_RE.captures(&date_string) {
        let unit = TimeUnit::parse(&m[2]).expect("unit from regex");
        let mut start = now;
        let mut end = now;
        if &m[1] == "last" {
            start = moment::add(start, -1, unit);
            end = moment::add(end, -1, unit);
        }
        if &m[1] == "next" {
            start = moment::add(start, 1, unit);
            end = moment::add(end, 1, unit);
        }
        let span = span_of(unit);
        [moment::start_of(start, span), moment::end_of(end, span)]
    } else if let Some(m) = LAST_NEXT_N_RE.captures(&date_string) {
        let unit = TimeUnit::parse(&m[3]).expect("unit from regex");
        let amount: i64 = m[2].parse().map_err(|_| cannot_parse(&date_string))?;
        let mut start = now;
        let mut end = now;
        if &m[1] == "last" {
            start = moment::add(start, -amount, unit);
            end = moment::add(end, -1, unit);
        }
        if &m[1] == "next" {
            start = moment::add(start, 1, unit);
            end = moment::add(end, amount, unit);
        }
        let span = span_of(unit);
        [moment::start_of(start, span), moment::end_of(end, span)]
    } else if date_string.contains("today") {
        [
            moment::start_of(now, TimeUnit::Day),
            moment::end_of(now, TimeUnit::Day),
        ]
    } else if date_string.contains("yesterday") {
        [
            moment::add(moment::start_of(now, TimeUnit::Day), -1, TimeUnit::Day),
            moment::add(moment::end_of(now, TimeUnit::Day), -1, TimeUnit::Day),
        ]
    } else if date_string.contains("tomorrow") {
        [
            moment::add(moment::start_of(now, TimeUnit::Day), 1, TimeUnit::Day),
            moment::add(moment::end_of(now, TimeUnit::Day), 1, TimeUnit::Day),
        ]
    } else if FROM_TO_RE.is_match(&date_string) {
        let m = FROM_TO_BOUNDED_RE
            .captures(&date_string)
            .ok_or_else(|| cannot_parse(&date_string))?;
        let from = m[1].trim();
        let to = m[2].trim();

        let reference = now.naive_local();
        let from_results = chrono_text::parse(from, reference);
        let to_results = chrono_text::parse(to, reference);

        let from_result = from_results.first().ok_or_else(|| cannot_parse(from))?;
        let to_result = to_results.first().ok_or_else(|| cannot_parse(to))?;

        let granularity = exact_granularity(&date_string);
        let start = components_in_tz(tz, &from_result.start, from)?;
        let end = components_in_tz(tz, &to_result.start, to)?;
        [
            moment::start_of(start, granularity),
            moment::end_of(end, granularity),
        ]
    } else {
        let reference = now.naive_local();
        let results = chrono_text::parse(&date_string, reference);
        let result = results.first().ok_or_else(|| cannot_parse(&date_string))?;

        let granularity = exact_granularity(&date_string);
        let start = components_in_tz(tz, &result.start, &date_string)?;
        let end = match &result.end {
            Some(end) => components_in_tz(tz, end, &date_string)?,
            None => start,
        };
        [
            moment::start_of(start, granularity),
            moment::end_of(end, granularity),
        ]
    };

    Ok([
        moment::format_local(&range[0]),
        moment::format_local(&range[1]),
    ])
}

/// `week` spans use ISO weeks (Monday based) in the date parser.
fn span_of(unit: TimeUnit) -> TimeUnit {
    match unit {
        TimeUnit::Week => TimeUnit::IsoWeek,
        other => other,
    }
}

/// The finest of `second`, `minute`, `hour` mentioned anywhere in the input
/// (in that order of precedence), defaulting to `day`.
fn exact_granularity(date_string: &str) -> TimeUnit {
    ["second", "minute", "hour"]
        .iter()
        .find(|g| date_string.contains(*g))
        .and_then(|g| TimeUnit::parse(g))
        .unwrap_or(TimeUnit::Day)
}

/// `momentFromResult`: interpret the parsed wall-clock components in `tz`.
fn components_in_tz(
    tz: Tz,
    components: &chrono_text::Components,
    text: &str,
) -> Result<DateTime<Tz>, QueryError> {
    let naive: NaiveDateTime = components.to_naive().ok_or_else(|| cannot_parse(text))?;
    Ok(moment::resolve_local(tz, naive))
}
