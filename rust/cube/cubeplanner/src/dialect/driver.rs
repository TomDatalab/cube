//! `DriverTools` for the dialects ported from `BaseQuery`'s subclasses.
//!
//! One struct with a per-dialect match, rather than one struct per dialect: the
//! JS subclasses differ in a handful of string builders each and share the rest
//! with `BaseQuery`, so the `match` arms read as the deltas they are and the
//! fall-through is the base behaviour.
//!
//! Postgres and CubeStore are not here — they keep the implementation the
//! planner's own fixtures already carry (`MockDriverTools`), so both crates
//! render them identically.

use crate::dialect::interval::{split_sql_interval, ParsedInterval};
use crate::dialect::Dialect;
use crate::error::UNSUPPORTED_BY_DIALECT;
use chrono::{Datelike, NaiveDate, NaiveDateTime, Offset, TimeZone};
use cubenativeutils::CubeError;
use cubesqlplanner::cube_bridge::driver_tools::DriverTools;
use cubesqlplanner::cube_bridge::sql_templates_render::SqlTemplatesRender;
use cubesqlplanner::rust_model::MockSqlTemplatesRender;
use std::any::Any;
use std::rc::Rc;

/// The non-template dialect behaviour for one request: a dialect plus the
/// request's timezone (baked into `convertTz` and friends) and that dialect's
/// template set.
#[derive(Clone)]
pub struct SqlDialectTools {
    dialect: Dialect,
    timezone: String,
    templates: Rc<MockSqlTemplatesRender>,
}

impl SqlDialectTools {
    pub fn new(dialect: Dialect, timezone: &str, templates: MockSqlTemplatesRender) -> Self {
        Self {
            dialect,
            timezone: timezone.to_string(),
            templates: Rc::new(templates),
        }
    }

    pub fn dialect(&self) -> Dialect {
        self.dialect
    }

    /// The zone's offset from UTC right now, in seconds.
    fn utc_offset_seconds(&self) -> Result<i32, CubeError> {
        if self.timezone == "UTC" {
            return Ok(0);
        }
        let tz: chrono_tz::Tz = self
            .timezone
            .parse()
            .map_err(|_| CubeError::user(format!("Unknown timezone {}", self.timezone)))?;
        Ok(tz
            .offset_from_utc_datetime(&chrono::Utc::now().naive_utc())
            .fix()
            .local_minus_utc())
    }

    /// `moment().tz(this.timezone).format('Z')` — the zone's offset right now,
    /// which is what MySQL's and MS SQL's `convertTz` interpolate when
    /// `CUBEJS_DB_*_USE_NAMED_TIMEZONES` is off (the default).
    fn utc_offset(&self) -> Result<String, CubeError> {
        Ok(Self::format_offset(self.utc_offset_seconds()?))
    }

    fn format_offset(seconds: i32) -> String {
        let sign = if seconds < 0 { '-' } else { '+' };
        let seconds = seconds.abs();
        format!("{}{:02}:{:02}", sign, seconds / 3600, (seconds % 3600) / 60)
    }

    /// `BaseQuery.diffTimeUnitForInterval` — the unit an interval is diffed in.
    /// A week is diffed in days and a quarter in months.
    fn diff_time_unit(interval: &str) -> &'static str {
        let interval = interval.to_lowercase();
        for (needle, unit) in [
            ("second", "second"),
            ("minute", "minute"),
            ("hour", "hour"),
            ("day", "day"),
            ("week", "day"),
            ("month", "month"),
            ("quarter", "month"),
        ] {
            if interval.contains(needle) {
                return unit;
            }
        }
        "year"
    }

    /// `MysqlQuery.tryFormatInterval` — MySQL's compound INTERVAL units only
    /// cover contiguous unit ranges, so an interval it cannot spell in one go
    /// has no single form.
    fn mysql_try_format_interval(interval: &str) -> Option<String> {
        let parsed = ParsedInterval::parse(interval);
        if let Some(v) = parsed.only("year") {
            return Some(format!("{v} YEAR"));
        }
        if let Some(v) = parsed.exactly(&["year", "month"]) {
            return Some(format!("'{}-{}' YEAR_MONTH", v[0], v[1]));
        }
        if let Some(v) = parsed.only("quarter") {
            return Some(format!("{v} QUARTER"));
        }
        if let Some(v) = parsed.only("month") {
            return Some(format!("{v} MONTH"));
        }
        if let Some(v) = parsed.only("week") {
            return Some(format!("{v} WEEK"));
        }
        if let Some(v) = parsed.only("day") {
            return Some(format!("{v} DAY"));
        }
        if let Some(v) = parsed.exactly(&["day", "hour"]) {
            return Some(format!("'{} {}' DAY_HOUR", v[0], v[1]));
        }
        if let Some(v) = parsed.exactly(&["day", "hour", "minute"]) {
            return Some(format!("'{} {}:{}' DAY_MINUTE", v[0], v[1], v[2]));
        }
        if let Some(v) = parsed.exactly(&["day", "hour", "minute", "second"]) {
            return Some(format!("'{} {}:{}:{}' DAY_SECOND", v[0], v[1], v[2], v[3]));
        }
        if let Some(v) = parsed.exactly(&["hour", "minute"]) {
            return Some(format!("'{}:{}' HOUR_MINUTE", v[0], v[1]));
        }
        if let Some(v) = parsed.exactly(&["hour", "minute", "second"]) {
            return Some(format!("'{}:{}:{}' HOUR_SECOND", v[0], v[1], v[2]));
        }
        if let Some(v) = parsed.exactly(&["minute", "second"]) {
            return Some(format!("'{}:{}' MINUTE_SECOND", v[0], v[1]));
        }
        if let Some(v) = parsed.only("hour") {
            return Some(format!("{v} HOUR"));
        }
        if let Some(v) = parsed.only("minute") {
            return Some(format!("{v} MINUTE"));
        }
        if let Some(v) = parsed.only("second") {
            return Some(format!("{v} SECOND"));
        }
        if let Some(v) = parsed.only("millisecond") {
            // MySQL has no MILLISECOND unit.
            return Some(format!("{} MICROSECOND", v * 1000));
        }
        None
    }

    fn mysql_format_interval(interval: &str) -> Result<String, CubeError> {
        Self::mysql_try_format_interval(interval).ok_or_else(|| {
            CubeError::user(format!(
                "Cannot transform interval expression \"{interval}\" to MySQL dialect"
            ))
        })
    }

    /// `MysqlQuery.applyInterval`: one unit at a time, coarsest first, when the
    /// whole interval has no single MySQL spelling.
    fn mysql_apply_interval(
        fn_name: &str,
        date: &str,
        interval: &str,
    ) -> Result<String, CubeError> {
        let parts = match Self::mysql_try_format_interval(interval) {
            Some(whole) => vec![whole],
            None => split_sql_interval(interval)
                .iter()
                .map(|part| Self::mysql_format_interval(part))
                .collect::<Result<Vec<_>, _>>()?,
        };
        Ok(parts.into_iter().fold(date.to_string(), |acc, part| {
            format!("{fn_name}({acc}, INTERVAL {part})")
        }))
    }

    /// `ClickHouseQuery.formatInterval` — a sum of single-unit intervals.
    fn clickhouse_format_interval(interval: &str) -> String {
        ParsedInterval::parse(interval)
            .parts()
            .iter()
            .map(|(unit, value)| format!("INTERVAL {} {}", value, unit.to_uppercase()))
            .collect::<Vec<_>>()
            .join(" + ")
    }

    /// `SnowflakeQuery.formatInterval`: `"2 years 3 months"` becomes
    /// `"2 years, 3 months"`.
    fn snowflake_format_interval(interval: &str) -> String {
        let words: Vec<&str> = interval.split(' ').collect();
        let last = words.len().saturating_sub(1);
        words
            .iter()
            .enumerate()
            .map(|(index, word)| {
                if index % 2 != 0 && index < last {
                    format!("{word},")
                } else {
                    (*word).to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// `BigqueryQuery.formatInterval` / `DatabricksQuery.formatInterval`:
    /// `(formatted interval, time unit for the DATEDIFF family)`. The two
    /// differ only in how each component is quoted and in which combinations
    /// they accept, so `quoted_single` selects between them.
    fn range_interval(
        interval: &str,
        dialect: &str,
        quoted_single: bool,
    ) -> Result<(String, String), CubeError> {
        let parsed = ParsedInterval::parse(interval);
        let single = |value: i64, unit: &str| -> (String, String) {
            let rendered = if quoted_single {
                format!("'{value}' {unit}")
            } else {
                format!("{value} {unit}")
            };
            (rendered, unit.to_string())
        };
        let range = |values: &[i64], unit: &str, sep: &[&str]| -> (String, String) {
            let mut out = values[0].to_string();
            for (index, value) in values.iter().enumerate().skip(1) {
                out.push_str(sep[index - 1]);
                out.push_str(&value.to_string());
            }
            // The DATEDIFF family counts in the range's finest unit:
            // `YEAR TO MONTH` is diffed in months (`formatInterval`).
            let finest = unit.rsplit(' ').next().unwrap_or(unit);
            (format!("'{out}' {unit}"), finest.to_string())
        };

        if let Some(v) = parsed.only("year") {
            return Ok(single(v, "YEAR"));
        }
        if let Some(v) = parsed.exactly(&["year", "month"]) {
            return Ok(range(&v, "YEAR TO MONTH", &["-"]));
        }
        if !quoted_single {
            // BigQuery accepts the wider YEAR TO … ranges, Databricks does not.
            if let Some(v) = parsed.exactly(&["year", "month", "day"]) {
                return Ok(range(&v, "YEAR TO DAY", &["-", " "]));
            }
            if let Some(v) = parsed.exactly(&["year", "month", "day", "hour"]) {
                return Ok(range(&v, "YEAR TO HOUR", &["-", " ", " "]));
            }
            if let Some(v) = parsed.exactly(&["year", "month", "day", "hour", "minute"]) {
                return Ok(range(&v, "YEAR TO MINUTE", &["-", " ", " ", ":"]));
            }
            if let Some(v) = parsed.exactly(&["year", "month", "day", "hour", "minute", "second"]) {
                return Ok(range(&v, "YEAR TO SECOND", &["-", " ", " ", ":", ":"]));
            }
            if let Some(v) = parsed.only("quarter") {
                return Ok(single(v, "QUARTER"));
            }
        }
        if let Some(v) = parsed.only("month") {
            return Ok(single(v, "MONTH"));
        }
        if !quoted_single {
            if let Some(v) = parsed.exactly(&["month", "day"]) {
                return Ok(range(&v, "MONTH TO DAY", &[" "]));
            }
            if let Some(v) = parsed.exactly(&["month", "day", "hour"]) {
                return Ok(range(&v, "MONTH TO HOUR", &[" ", " "]));
            }
            if let Some(v) = parsed.exactly(&["month", "day", "hour", "minute"]) {
                return Ok(range(&v, "MONTH TO MINUTE", &[" ", " ", ":"]));
            }
            if let Some(v) = parsed.exactly(&["month", "day", "hour", "minute", "second"]) {
                return Ok(range(&v, "MONTH TO SECOND", &[" ", " ", ":", ":"]));
            }
            if let Some(v) = parsed.only("week") {
                return Ok((format!("{v} WEEK"), "DAY".to_string()));
            }
        }
        if let Some(v) = parsed.only("day") {
            return Ok(single(v, "DAY"));
        }
        if let Some(v) = parsed.exactly(&["day", "hour"]) {
            return Ok(range(&v, "DAY TO HOUR", &[" "]));
        }
        if let Some(v) = parsed.exactly(&["day", "hour", "minute"]) {
            return Ok(range(&v, "DAY TO MINUTE", &[" ", ":"]));
        }
        if let Some(v) = parsed.exactly(&["day", "hour", "minute", "second"]) {
            return Ok(range(&v, "DAY TO SECOND", &[" ", ":", ":"]));
        }
        if let Some(v) = parsed.exactly(&["hour", "minute"]) {
            return Ok(range(&v, "HOUR TO MINUTE", &[":"]));
        }
        if let Some(v) = parsed.exactly(&["hour", "minute", "second"]) {
            return Ok(range(&v, "HOUR TO SECOND", &[":", ":"]));
        }
        if let Some(v) = parsed.exactly(&["minute", "second"]) {
            return Ok(range(&v, "MINUTE TO SECOND", &[":"]));
        }
        if let Some(v) = parsed.only("hour") {
            return Ok(single(v, "HOUR"));
        }
        if let Some(v) = parsed.only("minute") {
            return Ok(single(v, "MINUTE"));
        }
        if let Some(v) = parsed.only("second") {
            return Ok(single(v, "SECOND"));
        }
        if !quoted_single {
            if let Some(v) = parsed.only("millisecond") {
                return Ok((format!("'{v}' MILLISECOND"), "MILLISECOND".to_string()));
            }
        }
        Err(CubeError::user(format!(
            "Cannot transform interval expression \"{interval}\" to {dialect} dialect"
        )))
    }

    fn bigquery_interval(interval: &str) -> Result<(String, String), CubeError> {
        Self::range_interval(interval, "BigQuery", false)
    }

    fn databricks_interval(interval: &str) -> Result<(String, String), CubeError> {
        Self::range_interval(interval, "Databricks", true)
    }

    /// A construct this dialect's database has no spelling for. Surfaces as
    /// `PlannerError::Unsupported` (see [`UNSUPPORTED_BY_DIALECT`]).
    fn unsupported(&self, what: impl std::fmt::Display) -> CubeError {
        CubeError::user(format!(
            "{UNSUPPORTED_BY_DIALECT}{} dialect: {what}",
            self.dialect.as_str()
        ))
    }

    fn hll_unsupported(&self) -> CubeError {
        self.unsupported("distributed approximate distinct count (HLL rollups)")
    }

    /// `BaseQuery.dateBin` — custom granularities need a dialect's own binning.
    fn date_bin_unsupported(&self) -> CubeError {
        self.unsupported(
            "custom time dimension granularities (the date bin function is not implemented for this data source)",
        )
    }

    /// `PostgresQuery.intervalString`: quarters folded into months, units
    /// pluralised the way Postgres spells them.
    fn postgres_interval_string(interval: &str) -> String {
        let parsed = ParsedInterval::parse(interval);
        let mut parts: Vec<(String, i64)> = Vec::new();
        for (unit, value) in parsed.parts() {
            let (unit, value) = if unit == "quarter" {
                ("month".to_string(), value * 3)
            } else {
                (unit.clone(), *value)
            };
            match parts.iter_mut().find(|(u, _)| *u == unit) {
                Some(slot) => slot.1 += value,
                None => parts.push((unit, value)),
            }
        }
        let normalized = parts
            .iter()
            .map(|(unit, value)| format!("{value} {unit}{}", if *value != 1 { "s" } else { "" }))
            .collect::<Vec<_>>()
            .join(" ");
        format!("'{normalized}'")
    }

    /// `PrestodbQuery.intervalString`: `'<n>' <unit>`. A Presto INTERVAL has
    /// no WEEK or QUARTER field and no sub-second one, so those are spelled in
    /// days, months and fractional seconds. A compound interval becomes one
    /// YEAR TO MONTH or DAY TO SECOND literal; one mixing the two families has
    /// no single literal.
    fn presto_interval_string(&self, interval: &str) -> Result<String, CubeError> {
        let parsed = ParsedInterval::parse(interval);
        let mut months: i64 = 0;
        let mut millis: i64 = 0;
        let mut single: Option<(String, String)> = None;
        for (unit, value) in parsed.parts() {
            let (field, amount) = match unit.as_str() {
                "year" => {
                    months += value * 12;
                    ("year", value.to_string())
                }
                "quarter" => {
                    months += value * 3;
                    ("month", (value * 3).to_string())
                }
                "month" => {
                    months += value;
                    ("month", value.to_string())
                }
                "week" => {
                    millis += value * 7 * 86_400_000;
                    ("day", (value * 7).to_string())
                }
                "day" => {
                    millis += value * 86_400_000;
                    ("day", value.to_string())
                }
                "hour" => {
                    millis += value * 3_600_000;
                    ("hour", value.to_string())
                }
                "minute" => {
                    millis += value * 60_000;
                    ("minute", value.to_string())
                }
                "second" => {
                    millis += value * 1000;
                    ("second", value.to_string())
                }
                "millisecond" => {
                    millis += value;
                    ("second", Self::millis_as_seconds(*value))
                }
                other => return Err(self.unsupported(format!("interval unit `{other}`"))),
            };
            single = Some((amount, field.to_string()));
        }
        if parsed.len() == 1 {
            let (amount, field) = single.expect("one part");
            return Ok(format!("'{amount}' {field}"));
        }
        match (months != 0, millis != 0) {
            (true, true) => Err(self.unsupported(format!(
                "the interval `{interval}`, which mixes calendar and fixed-length units in one INTERVAL literal"
            ))),
            (true, false) => Ok(format!("'{months}' month")),
            (false, _) => {
                let sign = if millis < 0 { "-" } else { "" };
                let millis = millis.abs();
                let days = millis / 86_400_000;
                let rest = millis % 86_400_000;
                Ok(format!(
                    "'{sign}{days} {:02}:{:02}:{:02}.{:03}' day to second",
                    rest / 3_600_000,
                    (rest % 3_600_000) / 60_000,
                    (rest % 60_000) / 1000,
                    rest % 1000
                ))
            }
        }
    }

    /// `n` milliseconds as a decimal number of seconds: `1` is `0.001`.
    fn millis_as_seconds(millis: i64) -> String {
        let sign = if millis < 0 { "-" } else { "" };
        let millis = millis.abs();
        format!("{sign}{}.{:03}", millis / 1000, millis % 1000)
    }

    /// A single-unit interval for the dialects whose INTERVAL has no WEEK or
    /// QUARTER field: weeks become days and quarters months.
    fn without_week_and_quarter(unit: &str, value: i64) -> (&str, i64) {
        match unit {
            "week" => ("day", value * 7),
            "quarter" => ("month", value * 3),
            other => (other, value),
        }
    }

    /// The parts of `interval` in the order `splitSqlInterval` applies them.
    fn split_parts(interval: &str) -> Vec<(String, i64)> {
        split_sql_interval(interval)
            .iter()
            .flat_map(|part| ParsedInterval::parse(part).parts().to_vec())
            .collect()
    }

    /// `HiveQuery.applyInterval`, and Druid's, Dremio's, ksqlDB's, QuestDB's and
    /// SQLite's single-unit forms: one step per component, coarsest first.
    fn apply_interval_parts(
        &self,
        date: String,
        interval: &str,
        negate: bool,
    ) -> Result<String, CubeError> {
        let parts = Self::split_parts(interval);
        match self.dialect {
            Dialect::Sqlite => {
                // A strftime modifier carries one unit, so each component is
                // its own argument.
                let modifiers = parts
                    .iter()
                    .map(|(unit, value)| {
                        let value = if negate { -value } else { *value };
                        match unit.as_str() {
                            "millisecond" => Ok(format!(
                                "'{} seconds'",
                                Self::millis_as_seconds(value)
                            )),
                            "year" | "quarter" | "month" | "week" | "day" | "hour" | "minute"
                            | "second" => {
                                let (unit, value) = Self::without_week_and_quarter(unit, value);
                                Ok(format!("'{value} {unit}'"))
                            }
                            other => Err(self.unsupported(format!("interval unit `{other}`"))),
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(format!(
                    "strftime('%Y-%m-%dT%H:%M:%f', {date}, {})",
                    modifiers.join(", ")
                ))
            }
            _ => parts.iter().try_fold(date, |acc, (unit, value)| {
                let op = if negate { '-' } else { '+' };
                match self.dialect {
                    Dialect::Hive => {
                        let (unit, value) = Self::without_week_and_quarter(unit, *value);
                        if unit == "millisecond" {
                            Ok(format!(
                                "({acc} {op} INTERVAL '{}' second)",
                                Self::millis_as_seconds(value)
                            ))
                        } else {
                            Ok(format!("({acc} {op} INTERVAL '{value}' {unit})"))
                        }
                    }
                    Dialect::Druid => Ok(format!("({acc} {op} INTERVAL {value} {unit})")),
                    Dialect::Dremio => {
                        let (unit, value) = Self::without_week_and_quarter(unit, *value);
                        if !matches!(unit, "year" | "month" | "day" | "hour" | "minute" | "second")
                        {
                            return Err(self.unsupported(format!("interval unit `{unit}`")));
                        }
                        let function = if negate { "DATE_SUB" } else { "DATE_ADD" };
                        Ok(format!(
                            "{function}({acc}, CAST({value} as INTERVAL {}))",
                            unit.to_uppercase()
                        ))
                    }
                    Dialect::Ksql => {
                        // TIMESTAMPADD / TIMESTAMPSUB take java TimeUnits: nothing
                        // coarser than DAYS.
                        let (unit, value) = Self::without_week_and_quarter(unit, *value);
                        let time_unit = match unit {
                            "day" => "DAYS",
                            "hour" => "HOURS",
                            "minute" => "MINUTES",
                            "second" => "SECONDS",
                            "millisecond" => "MILLISECONDS",
                            other => {
                                return Err(self.unsupported(format!(
                                    "`{other}` interval arithmetic (TIMESTAMPADD takes no calendar units)"
                                )))
                            }
                        };
                        let function = if negate { "TIMESTAMPSUB" } else { "TIMESTAMPADD" };
                        Ok(format!("{function}({time_unit}, {value}, {acc})"))
                    }
                    Dialect::QuestDb => {
                        // `INTERVAL_TO_QUEST_DATE_UNIT`: single-character periods.
                        let (period, factor) = Self::quest_period(unit)
                            .ok_or_else(|| self.unsupported(format!("interval unit `{unit}`")))?;
                        let signed = if negate { -value } else { *value };
                        Ok(format!("dateadd('{period}', {}, {acc})", signed * factor))
                    }
                    other => Err(CubeError::internal(format!(
                        "No single-unit interval arithmetic for dialect {}",
                        other.as_str()
                    ))),
                }
            }),
        }
    }

    /// QuestDB's `dateadd` / `timestamp_floor` period for a unit, and the
    /// factor a value is scaled by (a quarter is three months).
    fn quest_period(unit: &str) -> Option<(&'static str, i64)> {
        Some(match unit {
            "millisecond" => ("T", 1),
            "second" => ("s", 1),
            "minute" => ("m", 1),
            "hour" => ("h", 1),
            "day" => ("d", 1),
            "week" => ("w", 1),
            "month" => ("M", 1),
            "quarter" => ("M", 3),
            "year" => ("y", 1),
            _ => return None,
        })
    }

    /// `OracleQuery.addInterval` / `subtractInterval`: ADD_MONTHS for the
    /// calendar units, NUMTODSINTERVAL for the rest. Weeks are seven days and
    /// milliseconds fractional seconds, where the JS drops them.
    fn oracle_apply_interval(date: String, interval: &str, negate: bool) -> String {
        let parsed = ParsedInterval::parse(interval);
        let get = |unit: &str| parsed.get(unit).unwrap_or(0);
        let months = get("year") * 12 + get("quarter") * 3 + get("month");
        let mut res = date;
        if months != 0 {
            let months = if negate { -months } else { months };
            res = format!("ADD_MONTHS({res}, {months})");
        }
        let op = if negate { '-' } else { '+' };
        let days = get("week") * 7 + get("day");
        for (value, unit) in [
            (days.to_string(), "DAY"),
            (get("hour").to_string(), "HOUR"),
            (get("minute").to_string(), "MINUTE"),
            (get("second").to_string(), "SECOND"),
        ] {
            if value != "0" {
                res = format!("{res} {op} NUMTODSINTERVAL({value}, '{unit}')");
            }
        }
        if get("millisecond") != 0 {
            res = format!(
                "{res} {op} NUMTODSINTERVAL({}, 'SECOND')",
                Self::millis_as_seconds(get("millisecond"))
            );
        }
        res
    }

    /// `PinotQuery.applyInterval`: fixed-length units as epoch-millis offsets,
    /// calendar units through TIMESTAMPADD.
    fn pinot_apply_interval(
        &self,
        date: String,
        interval: &str,
        sign: i64,
    ) -> Result<String, CubeError> {
        let mut expr = format!("CAST({date} as TIMESTAMP)");
        for (unit, raw) in ParsedInterval::parse(interval).parts() {
            let value = raw * sign;
            expr = match unit.as_str() {
                "second" | "minute" | "hour" | "day" | "week" => {
                    let amount = if unit == "week" { value * 7 } else { value };
                    let op = if amount < 0 { '-' } else { '+' };
                    let function = match unit.as_str() {
                        "second" => "fromEpochSeconds",
                        "minute" => "fromEpochMinutes",
                        "hour" => "fromEpochHours",
                        _ => "fromEpochDays",
                    };
                    format!("{expr} {op} {function}({})", amount.abs())
                }
                "month" => format!("TIMESTAMPADD(MONTH, {value}, {expr})"),
                "quarter" => format!("TIMESTAMPADD(MONTH, {}, {expr})", value * 3),
                "year" => format!("TIMESTAMPADD(YEAR, {value}, {expr})"),
                other => return Err(self.unsupported(format!("interval unit `{other}`"))),
            };
        }
        Ok(expr)
    }

    /// `QuestQuery.dateBin`: `timestamp_floor` only bins forward from its
    /// origin, so the origin is moved back a whole number of strides to just
    /// before a fixed anchor that precedes any realistic data.
    fn questdb_date_bin(
        &self,
        interval: &str,
        source: &str,
        origin: &str,
    ) -> Result<String, CubeError> {
        const ANCHOR_YEAR: i64 = 1000;
        let parsed = ParsedInterval::parse(interval);
        let (unit, duration) = match parsed.parts() {
            [(unit, duration)] if unit != "millisecond" => (unit.as_str(), *duration),
            _ => {
                return Err(self.unsupported(format!(
                    "the custom granularity interval `{interval}` (timestamp_floor takes a single unit)"
                )))
            }
        };
        let (period, factor) = Self::quest_period(unit)
            .ok_or_else(|| self.unsupported(format!("interval unit `{unit}`")))?;
        let count = duration * factor;
        let origin_time =
            NaiveDateTime::parse_from_str(origin.trim_end_matches('Z'), "%Y-%m-%dT%H:%M:%S%.f")
                .or_else(|_| {
                    NaiveDate::parse_from_str(origin, "%Y-%m-%d")
                        .map(|d| d.and_hms_opt(0, 0, 0).unwrap())
                })
                .map_err(|_| {
                    CubeError::user(format!(
                        "QuestDB custom granularity has an unparseable origin: {origin}"
                    ))
                })?;
        let anchor = NaiveDate::from_ymd_opt(ANCHOR_YEAR as i32, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap();
        let millis = (origin_time - anchor).num_milliseconds();
        // moment's `diff` in whole units, truncated toward zero.
        let months = (origin_time.year() as i64 - ANCHOR_YEAR) * 12 + origin_time.month0() as i64;
        let diff = match period {
            "M" => months,
            "y" => months / 12,
            "w" => millis / (7 * 86_400_000),
            "d" => millis / 86_400_000,
            "h" => millis / 3_600_000,
            "m" => millis / 60_000,
            _ => millis / 1000,
        };
        let strides = if diff > 0 {
            (diff + count - 1) / count
        } else {
            0
        };
        let shift = strides * count;
        if shift > i32::MAX as i64 {
            return Err(self.unsupported(format!(
                "custom granularity '{count} {period}': origin shift {shift} exceeds dateadd()'s 32-bit range"
            )));
        }
        let shifted = if shift > 0 {
            format!(
                "dateadd('{period}', {}, cast('{origin}' as timestamp))",
                -shift
            )
        } else {
            format!("cast('{origin}' as timestamp)")
        };
        Ok(format!(
            "timestamp_floor('{count}{period}', {source}, {shifted})"
        ))
    }
}

impl DriverTools for SqlDialectTools {
    fn as_any(self: Rc<Self>) -> Rc<dyn Any> {
        self
    }

    fn sql_templates(&self) -> Result<Rc<dyn SqlTemplatesRender>, CubeError> {
        Ok(self.templates.clone())
    }

    fn convert_tz(&self, field: String) -> Result<String, CubeError> {
        let tz = &self.timezone;
        Ok(match self.dialect {
            Dialect::MySql => format!(
                "CONVERT_TZ({field}, @@session.time_zone, '{}')",
                self.utc_offset()?
            ),
            // `MongoBiQuery.convertTz`: the BI Connector has no CONVERT_TZ, so
            // the zone's current offset is added by hand.
            Dialect::MongoBi => {
                let offset = self.utc_offset_seconds()?;
                let sign = offset.signum();
                let hours = offset / 3600;
                let minutes = sign * ((offset.abs() % 3600) / 60);
                let mut result = field;
                if hours != 0 {
                    result = format!("TIMESTAMPADD(HOUR, {hours}, {result})");
                }
                if minutes != 0 {
                    result = format!("TIMESTAMPADD(MINUTE, {minutes}, {result})");
                }
                result
            }
            Dialect::ClickHouse => format!("toTimeZone(toDateTime64({field}, 0), '{tz}')"),
            Dialect::BigQuery => format!("TIMESTAMP(DATETIME({field}, '{tz}'))"),
            Dialect::Snowflake => {
                format!("CONVERT_TIMEZONE('{tz}', {field}::timestamp_tz)::timestamp_ntz")
            }
            Dialect::Databricks | Dialect::Hive => format!("from_utc_timestamp({field}, '{tz}')"),
            Dialect::MsSql => format!(
                "CAST(SWITCHOFFSET(TODATETIMEOFFSET({field}, '+00:00'), '{}') AS DATETIME2)",
                self.utc_offset()?
            ),
            Dialect::Postgres | Dialect::CubeStore | Dialect::Redshift | Dialect::Crate => {
                format!("({field}::timestamptz AT TIME ZONE '{tz}')")
            }
            // `PrestodbQuery.convertTz`: a DATE is lifted to a timestamp with
            // COALESCE, which keeps a zoned timestamp's zone, and the zone's
            // offset is added by hand.
            Dialect::Presto => {
                let timestamp = format!("COALESCE({field}, CAST(NULL AS TIMESTAMP))");
                let at_timezone = format!("{timestamp} AT TIME ZONE '{tz}'");
                format!(
                    "CAST(date_add('minute', timezone_minute({at_timezone}), date_add('hour', timezone_hour({at_timezone}), {timestamp})) AS TIMESTAMP)"
                )
            }
            Dialect::Trino => format!(
                "CAST((COALESCE({field}, CAST(NULL AS TIMESTAMP)) AT TIME ZONE '{tz}') AS TIMESTAMP)"
            ),
            Dialect::Vertica | Dialect::Firebolt => format!("{field} AT TIME ZONE '{tz}'"),
            // `OracleQuery.convertTz` leaves the field as it is.
            Dialect::Oracle => field,
            // `SqliteQuery.convertTz` appends the offset with its sign flipped:
            // SQLite reads a `±HH:MM` suffix as the zone the time is in and
            // moves it to UTC, so the opposite offset lands on local time.
            Dialect::Sqlite => {
                let flipped = Self::format_offset(-self.utc_offset_seconds()?);
                let flipped = if self.utc_offset_seconds()? == 0 {
                    "-00:00".to_string()
                } else {
                    flipped
                };
                format!("{} || '{flipped}'", self.time_stamp_cast(field)?)
            }
            Dialect::Druid => format!(
                "CAST(TIME_FORMAT({field}, 'yyyy-MM-dd HH:mm:ss', '{tz}') AS TIMESTAMP)"
            ),
            Dialect::Dremio => format!("CONVERT_TIMEZONE('{tz}', {field})"),
            Dialect::Ksql => format!("CONVERT_TZ({field}, 'UTC', '{tz}')"),
            Dialect::QuestDb => format!("to_timezone({field}, '{tz}')"),
            Dialect::Pinot => format!(
                "CAST(toDateTime({field}, 'yyyy-MM-dd HH:mm:ss.SSS', '{tz}') as TIMESTAMP)"
            ),
            Dialect::DuckDb => format!("timezone('{tz}', {field}::timestamptz)"),
        })
    }

    fn time_grouped_column(
        &self,
        granularity: String,
        dimension: String,
    ) -> Result<String, CubeError> {
        let g = granularity.as_str();
        let d = dimension.as_str();
        let tz = &self.timezone;
        let unsupported = || self.unsupported(format!("the `{granularity}` granularity"));
        let standard = matches!(
            g,
            "day" | "week" | "hour" | "minute" | "second" | "month" | "quarter" | "year"
        );
        match self.dialect {
            Dialect::MySql | Dialect::MongoBi => {
                let inner = match g {
                    "day" => format!("DATE_FORMAT({d}, '%Y-%m-%dT00:00:00.000')"),
                    "week" => format!(
                        "DATE_FORMAT(DATE_ADD('1900-01-01', INTERVAL TIMESTAMPDIFF(WEEK, '1900-01-01', {d}) WEEK), '%Y-%m-%dT00:00:00.000')"
                    ),
                    "hour" => format!("DATE_FORMAT({d}, '%Y-%m-%dT%H:00:00.000')"),
                    "minute" => format!("DATE_FORMAT({d}, '%Y-%m-%dT%H:%i:00.000')"),
                    "second" => format!("DATE_FORMAT({d}, '%Y-%m-%dT%H:%i:%S.000')"),
                    "month" => format!("DATE_FORMAT({d}, '%Y-%m-01T00:00:00.000')"),
                    "quarter" => format!(
                        "DATE_ADD('1900-01-01', INTERVAL TIMESTAMPDIFF(QUARTER, '1900-01-01', {d}) QUARTER)"
                    ),
                    "year" => format!("DATE_FORMAT({d}, '%Y-01-01T00:00:00.000')"),
                    _ => return Err(unsupported()),
                };
                Ok(format!("CAST({inner} AS DATETIME)"))
            }
            Dialect::ClickHouse => {
                if g == "week" {
                    return Ok(format!("toDateTime64(toMonday({d}, '{tz}'), 0, '{tz}')"));
                }
                let interval = match g {
                    "day" => "Day",
                    "hour" => "Hour",
                    "minute" => "Minute",
                    "second" => "Second",
                    "month" => "Month",
                    "quarter" => "Quarter",
                    "year" => "Year",
                    _ => return Err(unsupported()),
                };
                let inner = if g == "second" {
                    format!("toDateTime64({d}, 0, '{tz}')")
                } else {
                    format!("toStartOf{interval}({d}, '{tz}')")
                };
                Ok(format!("toDateTime64({inner}, 0, '{tz}')"))
            }
            Dialect::BigQuery => {
                let interval = match g {
                    "day" => "DAY",
                    "week" => "WEEK(MONDAY)",
                    "hour" => "HOUR",
                    "minute" => "MINUTE",
                    "second" => "SECOND",
                    "month" => "MONTH",
                    "quarter" => "QUARTER",
                    "year" => "YEAR",
                    _ => return Err(unsupported()),
                };
                self.time_stamp_cast(format!("DATETIME_TRUNC({d}, {interval})"))
            }
            Dialect::Snowflake => {
                if !standard {
                    return Err(unsupported());
                }
                Ok(format!("date_trunc('{}', {d})", g.to_uppercase()))
            }
            Dialect::MsSql => Ok(match g {
                "second" => format!("CAST(FORMAT({d}, 'yyyy-MM-ddTHH:mm:ss.000') AS DATETIME2)"),
                "day" | "week" | "hour" | "minute" | "month" | "quarter" | "year" => {
                    format!("dateadd({g}, DATEDIFF({g}, 0, {d}), 0)")
                }
                _ => return Err(unsupported()),
            }),
            Dialect::Postgres
            | Dialect::CubeStore
            | Dialect::Databricks
            | Dialect::Redshift
            | Dialect::Crate
            | Dialect::Presto
            | Dialect::Trino => {
                if !standard {
                    return Err(unsupported());
                }
                Ok(format!("date_trunc('{g}', {d})"))
            }
            Dialect::Druid | Dialect::Dremio | Dialect::DuckDb => {
                if !standard {
                    return Err(unsupported());
                }
                Ok(format!("DATE_TRUNC('{g}', {d})"))
            }
            Dialect::Firebolt => {
                if !standard {
                    return Err(unsupported());
                }
                Ok(format!("DATE_TRUNC('{}', {d})", g.to_uppercase()))
            }
            // `VerticaQuery` and `OracleQuery` truncate with format models,
            // which are case-insensitive: minutes are `MI` (`mm` is the month)
            // and ISO weeks, starting on Monday, are `IW` (`W` is the weekday
            // of the month's first day).
            Dialect::Vertica => {
                let model = match g {
                    "day" => "DD",
                    "week" => "IW",
                    "hour" => "HH24",
                    "minute" => "MI",
                    "second" => "SS",
                    "month" => "MM",
                    "quarter" => "Q",
                    "year" => "YY",
                    _ => return Err(unsupported()),
                };
                Ok(format!("TRUNC({d}, '{model}')"))
            }
            Dialect::Oracle => {
                let model = match g {
                    "day" => "DD",
                    "week" => "IW",
                    "hour" => "HH24",
                    "minute" => "MI",
                    // TRUNC has no seconds model; a DATE holds whole seconds.
                    "second" => return Ok(format!("CAST({d} AS DATE)")),
                    "month" => "MM",
                    "quarter" => "Q",
                    "year" => "YYYY",
                    _ => return Err(unsupported()),
                };
                Ok(format!("TRUNC({d}, '{model}')"))
            }
            Dialect::Hive => Ok(match g {
                "day" => format!("DATE_FORMAT({d}, 'yyyy-MM-dd 00:00:00.000')"),
                "week" => format!(
                    "DATE_FORMAT(from_unixtime(unix_timestamp('1900-01-01 00:00:00') + floor((unix_timestamp({d}) - unix_timestamp('1900-01-01 00:00:00')) / (60 * 60 * 24 * 7)) * (60 * 60 * 24 * 7)), 'yyyy-MM-dd 00:00:00.000')"
                ),
                "hour" => format!("DATE_FORMAT({d}, 'yyyy-MM-dd HH:00:00.000')"),
                "minute" => format!("DATE_FORMAT({d}, 'yyyy-MM-dd HH:mm:00.000')"),
                "second" => format!("DATE_FORMAT({d}, 'yyyy-MM-dd HH:mm:ss.000')"),
                "month" => format!("DATE_FORMAT({d}, 'yyyy-MM-01 00:00:00.000')"),
                "year" => format!("DATE_FORMAT({d}, 'yyyy-01-01 00:00:00.000')"),
                // `HiveQuery`'s GRANULARITY_TO_INTERVAL has no quarter.
                _ => return Err(unsupported()),
            }),
            Dialect::Sqlite => Ok(match g {
                "day" => format!("strftime('%Y-%m-%dT00:00:00.000', {d})"),
                "week" => format!(
                    "strftime('%Y-%m-%dT00:00:00.000', CASE WHEN date({d}, 'weekday 1') = date({d}) THEN date({d}, 'weekday 1') ELSE date({d}, 'weekday 1', '-7 days') END)"
                ),
                "hour" => format!("strftime('%Y-%m-%dT%H:00:00.000', {d})"),
                "minute" => format!("strftime('%Y-%m-%dT%H:%M:00.000', {d})"),
                "second" => format!("strftime('%Y-%m-%dT%H:%M:%S.000', {d})"),
                "month" => format!("strftime('%Y-%m-01T00:00:00.000', {d})"),
                "year" => format!("strftime('%Y-01-01T00:00:00.000', {d})"),
                "quarter" => format!(
                    "CASE\n      WHEN cast(strftime('%m', {d}) as integer) BETWEEN 1 AND 3 THEN strftime('%Y-01-01T00:00:00.000', {d})\n      WHEN cast(strftime('%m', {d}) as integer) BETWEEN 4 AND 6 THEN strftime('%Y-04-01T00:00:00.000', {d})\n      WHEN cast(strftime('%m', {d}) as integer) BETWEEN 7 AND 9 THEN strftime('%Y-07-01T00:00:00.000', {d})\n      ELSE strftime('%Y-10-01T00:00:00.000', {d})\n    END"
                ),
                _ => return Err(unsupported()),
            }),
            Dialect::Ksql => {
                let formatted = match g {
                    "day" => format!("FORMAT_TIMESTAMP({d}, 'yyyy-MM-dd''T''00:00:00.000')"),
                    "week" => format!(
                        "FORMAT_TIMESTAMP(PARSE_TIMESTAMP(FORMAT_TIMESTAMP({d}, 'YYYY-ww'), 'YYYY-ww'), 'yyyy-MM-dd''T''00:00:00.000')"
                    ),
                    "hour" => format!("FORMAT_TIMESTAMP({d}, 'yyyy-MM-dd''T''HH:00:00.000')"),
                    "minute" => format!("FORMAT_TIMESTAMP({d}, 'yyyy-MM-dd''T''HH:mm:00.000')"),
                    "second" => format!("FORMAT_TIMESTAMP({d}, 'yyyy-MM-dd''T''HH:mm:ss.000')"),
                    "month" => format!("FORMAT_TIMESTAMP({d}, 'yyyy-MM-01''T''00:00:00.000')"),
                    "quarter" => format!(
                        "FORMAT_TIMESTAMP(PARSE_TIMESTAMP(FORMAT_TIMESTAMP({d}, 'YYYY-qq'), 'YYYY-qq'), 'yyyy-MM-dd''T''00:00:00.000')"
                    ),
                    "year" => format!("FORMAT_TIMESTAMP({d}, 'yyyy-01-01''T''00:00:00.000')"),
                    _ => return Err(unsupported()),
                };
                Ok(format!(
                    "PARSE_TIMESTAMP({formatted}, 'yyyy-MM-dd''T''HH:mm:ss.SSS', 'UTC')"
                ))
            }
            Dialect::QuestDb => {
                let period = match g {
                    "second" => "s",
                    "minute" => "m",
                    "hour" => "h",
                    "day" => "d",
                    "week" => "w",
                    "month" => "M",
                    "year" => "y",
                    // `QuestQuery.timeGroupedColumn` has no quarter period.
                    _ => return Err(unsupported()),
                };
                Ok(format!("timestamp_floor('{period}', {d})"))
            }
            Dialect::Pinot => {
                if !standard {
                    return Err(unsupported());
                }
                Ok(format!("CAST(dateTrunc('{g}', {d}) as TIMESTAMP)"))
            }
        }
    }

    fn timestamp_precision(&self) -> Result<u32, CubeError> {
        Ok(match self.dialect {
            Dialect::BigQuery => 6,
            _ => 3,
        })
    }

    fn time_stamp_cast(&self, field: String) -> Result<String, CubeError> {
        Ok(match self.dialect {
            Dialect::MySql => {
                format!("TIMESTAMP(convert_tz({field}, '+00:00', @@session.time_zone))")
            }
            Dialect::MongoBi => format!("TIMESTAMP({field})"),
            Dialect::ClickHouse => format!("parseDateTimeBestEffort({field})"),
            Dialect::BigQuery => format!("TIMESTAMP({field})"),
            Dialect::Snowflake => format!("{field}::timestamp_tz"),
            Dialect::Databricks | Dialect::Hive => {
                format!("from_utc_timestamp(replace(replace({field}, 'T', ' '), 'Z', ''), 'UTC')")
            }
            Dialect::MsSql => format!("CAST({field} AS DATETIMEOFFSET)"),
            Dialect::Postgres
            | Dialect::CubeStore
            | Dialect::Redshift
            | Dialect::Crate
            | Dialect::Vertica
            | Dialect::Firebolt
            | Dialect::DuckDb => format!("{field}::timestamptz"),
            Dialect::Presto | Dialect::Trino => format!("from_iso8601_timestamp({field})"),
            // `OracleQuery.timeStampCast` wraps its argument in `:"…"`, the
            // Oracle driver's spelling of a bind placeholder. The planner casts
            // literals through here too, and the driver binds a bare `?` the
            // same way, so the value is passed as it is.
            Dialect::Oracle => {
                format!("TO_TIMESTAMP_TZ({field}, 'YYYY-MM-DD\"T\"HH24:MI:SS.FF\"Z\"')")
            }
            Dialect::Sqlite => format!("strftime('%Y-%m-%dT%H:%M:%f', {field})"),
            Dialect::Druid => format!("TIME_PARSE({field})"),
            Dialect::Dremio => format!("TO_TIMESTAMP({field}, 'YYYY-MM-DD\"T\"HH24:MI:SS.FFF')"),
            Dialect::Ksql | Dialect::Pinot => format!("CAST({field} as TIMESTAMP)"),
            Dialect::QuestDb => field,
        })
    }

    fn date_time_cast(&self, field: String) -> Result<String, CubeError> {
        Ok(match self.dialect {
            Dialect::MySql | Dialect::MongoBi => format!("TIMESTAMP({field})"),
            Dialect::ClickHouse => format!("parseDateTimeBestEffort({field})"),
            Dialect::BigQuery => format!("DATETIME(TIMESTAMP({field}))"),
            Dialect::Databricks | Dialect::Hive => format!("from_utc_timestamp({field}, 'UTC')"),
            Dialect::MsSql => format!("CAST({field} AS DATETIME2)"),
            Dialect::Snowflake
            | Dialect::Postgres
            | Dialect::CubeStore
            | Dialect::Redshift
            | Dialect::Crate
            | Dialect::Vertica
            | Dialect::Druid
            | Dialect::DuckDb => format!("{field}::timestamp"),
            Dialect::Presto | Dialect::Trino => format!("from_iso8601_timestamp({field})"),
            Dialect::Oracle => format!(
                "CAST(TO_TIMESTAMP_TZ({field}, 'YYYY-MM-DD\"T\"HH24:MI:SS.FF\"Z\"') AS DATE)"
            ),
            Dialect::Sqlite => format!("strftime('%Y-%m-%dT%H:%M:%f', {field})"),
            Dialect::Firebolt => format!("{field}::timestampntz"),
            Dialect::Dremio => format!("TO_TIMESTAMP({field}, 'YYYY-MM-DD\"T\"HH24:MI:SS.FFF')"),
            Dialect::Ksql => format!("CAST({field} AS TIMESTAMP)"),
            Dialect::QuestDb | Dialect::Pinot => field,
        })
    }

    fn in_db_time_zone(&self, date: String) -> Result<String, CubeError> {
        if self.timezone == "UTC" {
            return Ok(date);
        }
        let tz: chrono_tz::Tz = self
            .timezone
            .parse()
            .map_err(|_| CubeError::user(format!("Unknown timezone {}", self.timezone)))?;
        let trimmed = date.trim_end_matches('Z');
        let naive = NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S%.f")
            .map_err(|e| CubeError::user(format!("Cannot parse timestamp '{date}': {e}")))?;
        let local = tz
            .from_local_datetime(&naive)
            .single()
            .ok_or_else(|| CubeError::user(format!("Ambiguous local time: {date}")))?;
        let utc = local.with_timezone(&chrono::Utc);
        Ok(if self.timestamp_precision()? == 3 {
            utc.format("%Y-%m-%dT%H:%M:%S%.3f").to_string()
        } else {
            utc.format("%Y-%m-%dT%H:%M:%S%.6f").to_string()
        })
    }

    fn get_allocated_params(&self) -> Result<Vec<String>, CubeError> {
        Ok(Vec::new())
    }

    fn should_reuse_params(&self) -> Result<bool, CubeError> {
        // `BaseQuery.shouldReuseParams` is false; `PostgresQuery` opts in, and
        // its subclasses with it. QuestDB numbers its placeholders too but
        // extends `BaseQuery`, so it binds one value per placeholder.
        Ok(matches!(self.dialect, Dialect::Redshift | Dialect::Crate))
    }

    fn interval_string(&self, interval: String) -> Result<String, CubeError> {
        Ok(match self.dialect {
            Dialect::MySql | Dialect::MongoBi => Self::mysql_format_interval(&interval)?,
            // `BigqueryQuery.intervalString` passes the interval through bare.
            Dialect::BigQuery => interval,
            Dialect::Redshift | Dialect::Crate => Self::postgres_interval_string(&interval),
            Dialect::Presto | Dialect::Trino => self.presto_interval_string(&interval)?,
            _ => format!("'{interval}'"),
        })
    }

    fn subtract_interval(&self, date: String, interval: String) -> Result<String, CubeError> {
        self.apply_interval(date, &interval, true)
    }

    fn add_interval(&self, date: String, interval: String) -> Result<String, CubeError> {
        self.apply_interval(date, &interval, false)
    }

    fn add_timestamp_interval(&self, date: String, interval: String) -> Result<String, CubeError> {
        self.add_interval(date, interval)
    }

    fn interval_and_minimal_time_unit(&self, interval: String) -> Result<Vec<String>, CubeError> {
        match self.dialect {
            // `BigqueryQuery.intervalAndMinimalTimeUnit` is `formatInterval`.
            Dialect::BigQuery => {
                let (formatted, unit) = Self::bigquery_interval(&interval)?;
                Ok(vec![formatted, unit])
            }
            // `SnowflakeQuery.intervalAndMinimalTimeUnit` keeps the interval's
            // own unit: DATEADD steps by whole units, so degrading WEEK to DAY
            // would produce seven times as many periods.
            Dialect::Snowflake => {
                const UNITS: [&str; 8] = [
                    "second", "minute", "hour", "day", "week", "month", "quarter", "year",
                ];
                let lower = interval.to_lowercase();
                let unit = UNITS
                    .iter()
                    .find(|unit| {
                        lower
                            .split(|c: char| !c.is_ascii_alphabetic())
                            .any(|word| word == **unit || word == format!("{unit}s"))
                    })
                    .copied()
                    .unwrap_or("year");
                Ok(vec![interval, unit.to_string()])
            }
            _ => {
                let unit = Self::diff_time_unit(&interval).to_string();
                Ok(vec![interval, unit])
            }
        }
    }

    fn hll_init(&self, sql: String) -> Result<String, CubeError> {
        match self.dialect {
            Dialect::BigQuery => Ok(format!("HLL_COUNT.INIT({sql})")),
            Dialect::Snowflake => Ok(format!("HLL_EXPORT(HLL_ACCUMULATE({sql}))")),
            Dialect::Databricks => Ok(format!("hll_sketch_agg({sql})")),
            // `PostgresQuery.hllInit`, inherited unchanged.
            Dialect::Redshift => Ok(format!("hll_add_agg(hll_hash_any({sql}))")),
            Dialect::Presto | Dialect::Trino => Ok(format!("cast(approx_set({sql}) as varbinary)")),
            _ => Err(self.hll_unsupported()),
        }
    }

    fn hll_merge(&self, sql: String) -> Result<String, CubeError> {
        match self.dialect {
            Dialect::BigQuery => Ok(format!("HLL_COUNT.MERGE({sql})")),
            Dialect::Snowflake => Ok(format!("HLL_ESTIMATE(HLL_COMBINE(HLL_IMPORT({sql})))")),
            Dialect::Databricks => Ok(format!("hll_union_agg({sql})")),
            Dialect::Redshift => Ok(format!("round(hll_cardinality(hll_union_agg({sql})))")),
            Dialect::Presto | Dialect::Trino => {
                Ok(format!("cardinality(merge(cast({sql} as HyperLogLog)))"))
            }
            _ => Err(self.hll_unsupported()),
        }
    }

    fn hll_cardinality_merge(&self, sql: String) -> Result<String, CubeError> {
        match self.dialect {
            // `DatabricksQuery.hllCardinalityMerge` is its own; everywhere else
            // `BaseQuery.hllCardinalityMerge` delegates to `hllMerge`.
            Dialect::Databricks => Ok(format!("hll_sketch_estimate(hll_union_agg({sql}))")),
            _ => self.hll_merge(sql),
        }
    }

    fn count_distinct_approx(&self, sql: String) -> Result<String, CubeError> {
        match self.dialect {
            Dialect::ClickHouse => Ok(format!("uniq({sql})")),
            Dialect::BigQuery | Dialect::Snowflake => Ok(format!("APPROX_COUNT_DISTINCT({sql})")),
            Dialect::Databricks | Dialect::DuckDb | Dialect::QuestDb => {
                Ok(format!("approx_count_distinct({sql})"))
            }
            Dialect::Redshift => Ok(format!(
                "round(hll_cardinality(hll_add_agg(hll_hash_any({sql}))))"
            )),
            Dialect::Crate => Ok(format!("hyperloglog_distinct({sql})")),
            Dialect::Presto | Dialect::Trino => Ok(format!("approx_distinct({sql})")),
            Dialect::Pinot => Ok(format!("DistinctCountHLLPlus({sql})")),
            _ => Err(self.unsupported("approximate distinct count")),
        }
    }

    fn support_generated_series_for_custom_td(&self) -> Result<bool, CubeError> {
        Ok(matches!(
            self.dialect,
            // MySQL follows CUBEJS_DB_MYSQL_USE_GENERATED_TIME_SERIES, which
            // defaults to true.
            Dialect::MySql
                | Dialect::Databricks
                | Dialect::Postgres
                | Dialect::Redshift
                | Dialect::Crate
                | Dialect::Presto
                | Dialect::Trino
        ))
    }

    fn date_bin(
        &self,
        interval: String,
        source: String,
        origin: String,
    ) -> Result<String, CubeError> {
        match self.dialect {
            Dialect::MySql | Dialect::MongoBi => {
                let formatted = Self::mysql_format_interval(&interval)?;
                let time_unit = if interval.to_lowercase().contains("year")
                    || interval.to_lowercase().contains("month")
                    || interval.to_lowercase().contains("quarter")
                {
                    "MONTH"
                } else {
                    "SECOND"
                };
                let origin_cast = self.date_time_cast(format!("'{origin}'"))?;
                let step = format!(
                    "TIMESTAMPDIFF({time_unit}, '1970-01-01 00:00:00', '1970-01-01 00:00:00' + INTERVAL {formatted})"
                );
                Ok(format!(
                    "TIMESTAMPADD({time_unit},\n        FLOOR(\n          TIMESTAMPDIFF({time_unit}, {origin_cast}, {source}) /\n          {step}\n        ) * {step},\n        {origin_cast}\n    )"
                ))
            }
            Dialect::ClickHouse => {
                let origin_aligned = format!("toDateTime64('{origin}', 3, '{}')", self.timezone);
                let formatted = Self::clickhouse_format_interval(&interval);
                let time_unit = Self::diff_time_unit(&interval);
                let begin = "fromUnixTimestamp(0)";
                Ok(format!(
                    "date_add({time_unit},\n        FLOOR(\n          date_diff({time_unit}, {origin_aligned}, {source}) /\n          date_diff({time_unit}, {begin}, {begin} + {formatted})\n        ) * date_diff({time_unit}, {begin}, {begin} + {formatted}),\n        {origin_aligned}\n    )"
                ))
            }
            Dialect::BigQuery => {
                let (formatted, time_unit) = Self::bigquery_interval(&interval)?;
                let begin = self.date_time_cast("'1970-01-01T00:00:00'".to_string())?;
                let origin_cast = self.date_time_cast(format!("'{origin}'"))?;
                let source_cast = self.date_time_cast(source)?;
                Ok(format!(
                    "({origin_cast} + INTERVAL {formatted} *\n      CAST(FLOOR(\n        DATETIME_DIFF({source_cast}, {origin_cast}, {time_unit}) /\n        DATETIME_DIFF({begin} + INTERVAL {formatted}, {begin}, {time_unit})\n      ) AS INT64))"
                ))
            }
            Dialect::Snowflake => {
                let formatted = Self::snowflake_format_interval(&interval);
                let time_unit = Self::diff_time_unit(&interval);
                let begin = "TIMESTAMP_FROM_PARTS(1970, 1, 1, 0, 0, 0)";
                let origin_cast = self.date_time_cast(format!("'{origin}'"))?;
                Ok(format!(
                    "DATEADD({time_unit},\n        FLOOR(\n          DATEDIFF({time_unit}, {origin_cast}, {source}) /\n          DATEDIFF({time_unit}, {begin}, ({begin} + interval '{formatted}'))\n        ) * DATEDIFF({time_unit}, {begin}, ({begin} + interval '{formatted}')),\n        {origin_cast})"
                ))
            }
            Dialect::Databricks => {
                let (formatted, time_unit) = Self::databricks_interval(&interval)?;
                let begin = self.date_time_cast("'1970-01-01T00:00:00'".to_string())?;
                let origin_cast = self.date_time_cast(format!("'{origin}'"))?;
                Ok(format!(
                    "{origin_cast} + INTERVAL {formatted} *\n      floor(\n        date_diff({time_unit}, {origin_cast}, {source}) /\n        date_diff({time_unit}, {begin}, {begin} + INTERVAL {formatted})\n      )"
                ))
            }
            Dialect::MsSql => {
                let origin_aligned = self.date_time_cast(format!("'{origin}'"))?;
                let begin = self.date_time_cast("DATEFROMPARTS(1970, 1, 1)".to_string())?;
                let time_unit = Self::diff_time_unit(&interval);
                let stepped = self.add_interval(begin.clone(), interval)?;
                Ok(format!(
                    "DATEADD({time_unit},\n        FLOOR(\n          CAST(DATEDIFF({time_unit}, {origin_aligned}, {source}) AS FLOAT) /\n          DATEDIFF({time_unit}, {begin}, {stepped})\n        ) * DATEDIFF({time_unit}, {begin}, {stepped}),\n        {origin_aligned}\n    )"
                ))
            }
            Dialect::Postgres | Dialect::CubeStore => {
                let interval_str = self.interval_string(interval)?;
                Ok(Self::postgres_date_bin(&interval_str, &source, &origin))
            }
            Dialect::Crate => {
                let interval_str = self.interval_string(interval)?;
                Ok(Self::postgres_date_bin(&interval_str, &source, &origin))
            }
            // `RedshiftQuery.dateBin`: Redshift intervals carry no month or
            // year part, so calendar intervals are binned in whole months.
            Dialect::Redshift => {
                let parsed = ParsedInterval::parse(&interval);
                let get = |unit: &str| parsed.get(unit).unwrap_or(0);
                let calendar = get("year") != 0 || get("month") != 0 || get("quarter") != 0;
                let fixed = ["week", "day", "hour", "minute", "second"]
                    .iter()
                    .any(|unit| get(unit) != 0);
                if calendar && fixed {
                    return Err(self.unsupported(format!(
                        "complex intervals like \"{interval}\"; use year to month or day to second intervals"
                    )));
                }
                if calendar {
                    let total = get("year") * 12 + get("quarter") * 3 + get("month");
                    let origin_cast = self.date_time_cast(format!("'{origin}'"))?;
                    return Ok(format!(
                        "DATEADD(\n      month,\n      (FLOOR(DATEDIFF(month, {origin_cast}, {source}) / {total}) * {total})::int,\n      {origin_cast}\n    )"
                    ));
                }
                let interval_str = self.interval_string(interval)?;
                Ok(Self::postgres_date_bin(&interval_str, &source, &origin))
            }
            // `PrestodbQuery.dateBin`: no INTERVAL arithmetic, so the bin is
            // counted with date_diff and stepped with date_add. The origin is
            // a plain TIMESTAMP so the result is not zoned.
            Dialect::Presto | Dialect::Trino => {
                let parsed = ParsedInterval::parse(&interval);
                let [(unit, count)] = parsed.parts() else {
                    return Err(self.unsupported(format!(
                        "the custom granularity interval `{interval}` (only simple intervals with one date part)"
                    )));
                };
                let origin_expr = format!(
                    "CAST({} AS TIMESTAMP)",
                    self.time_stamp_cast(format!("'{origin}'"))?
                );
                Ok(format!(
                    "date_add('{unit}',\n      floor(\n        date_diff('{unit}', {origin_expr}, {source}) / {count}\n      ) * {count},\n      {origin_expr}\n    )"
                ))
            }
            // `OracleQuery.dateBin`: month arithmetic for calendar intervals,
            // second arithmetic for fixed-length ones.
            Dialect::Oracle => {
                let parsed = ParsedInterval::parse(&interval);
                let get = |unit: &str| parsed.get(unit).unwrap_or(0);
                let origin_ts =
                    format!("TO_TIMESTAMP('{origin}', 'YYYY-MM-DD\"T\"HH24:MI:SS.FF3')");
                let months = get("year") * 12 + get("quarter") * 3 + get("month");
                let seconds = get("week") * 604_800
                    + get("day") * 86_400
                    + get("hour") * 3600
                    + get("minute") * 60
                    + get("second");
                if months > 0 && seconds == 0 {
                    return Ok(format!(
                        "ADD_MONTHS({origin_ts}, FLOOR(MONTHS_BETWEEN({source}, {origin_ts}) / {months}) * {months})"
                    ));
                }
                if seconds > 0 && months == 0 {
                    let diff =
                        format!("(CAST({source} AS DATE) - CAST({origin_ts} AS DATE)) * 86400");
                    return Ok(format!(
                        "{origin_ts} + NUMTODSINTERVAL(FLOOR({diff} / {seconds}) * {seconds}, 'SECOND')"
                    ));
                }
                Err(self.unsupported(format!(
                    "mixed month/second intervals in custom granularities: {interval}"
                )))
            }
            Dialect::QuestDb => self.questdb_date_bin(&interval, &source, &origin),
            // `PinotQuery.dateBin`.
            Dialect::Pinot => {
                let origin_aligned =
                    self.time_stamp_cast(format!("'{}'", origin.replace('T', " ")))?;
                let begin = self.time_stamp_cast("'1970-01-01 00:00:00.000'".to_string())?;
                let time_unit = Self::diff_time_unit(&interval).to_uppercase();
                let stepped = self.add_interval(begin.clone(), interval)?;
                let size = format!("TIMESTAMPDIFF({time_unit}, {begin}, {stepped})");
                self.time_stamp_cast(format!(
                    "TIMESTAMPADD({time_unit}, CAST(FLOOR(CAST(TIMESTAMPDIFF({time_unit}, {origin_aligned}, {source}) AS DOUBLE) / {size}) AS INTEGER) * {size}, {origin_aligned})"
                ))
            }
            // `DuckDBQuery.dateBin`: whole intervals, so bins stay aligned to
            // calendar months.
            Dialect::DuckDb => {
                let time_unit = Self::diff_time_unit(&interval);
                let begin = self.date_time_cast("'1970-01-01 00:00:00.000'".to_string())?;
                let origin_cast = self.date_time_cast(format!("'{origin}'"))?;
                Ok(format!(
                    "{origin_cast} + INTERVAL '{interval}' *\n      floor(\n        date_diff('{time_unit}', {origin_cast}, {source}) /\n        date_diff('{time_unit}', {begin}, {begin} + INTERVAL '{interval}')\n      )::int"
                ))
            }
            Dialect::Vertica
            | Dialect::Hive
            | Dialect::Sqlite
            | Dialect::Druid
            | Dialect::Firebolt
            | Dialect::Dremio
            | Dialect::Ksql => Err(self.date_bin_unsupported()),
        }
    }
}

impl SqlDialectTools {
    /// `subtractInterval` (`negate`) / `addInterval`, per dialect.
    fn apply_interval(
        &self,
        date: String,
        interval: &str,
        negate: bool,
    ) -> Result<String, CubeError> {
        let (sign, op, direction) = if negate {
            (-1, '-', "SUB")
        } else {
            (1, '+', "ADD")
        };
        Ok(match self.dialect {
            Dialect::MySql | Dialect::MongoBi => Self::mysql_apply_interval(
                if negate { "DATE_SUB" } else { "DATE_ADD" },
                &date,
                interval,
            )?,
            Dialect::ClickHouse => split_sql_interval(interval).iter().fold(date, |acc, part| {
                let function = if negate { "subDate" } else { "addDate" };
                format!(
                    "{function}({acc}, {})",
                    Self::clickhouse_format_interval(part)
                )
            }),
            Dialect::Snowflake => format!(
                "{date} {op} interval '{}'",
                Self::snowflake_format_interval(interval)
            ),
            Dialect::BigQuery => self.bigquery_apply_interval(direction, date, interval)?,
            Dialect::Databricks => ParsedInterval::parse(interval)
                .parts()
                .iter()
                .fold(date, |acc, (unit, value)| {
                    format!("({acc} {op} INTERVAL '{value}' {unit})")
                }),
            Dialect::MsSql => ParsedInterval::parse(interval)
                .parts()
                .iter()
                .fold(date, |acc, (unit, value)| {
                    format!("DATEADD({unit}, {}, {acc})", sign * value)
                }),
            // `RedshiftQuery.addInterval`: DATEADD per part, since Redshift
            // intervals carry no month or year part.
            Dialect::Redshift => {
                ParsedInterval::parse(interval)
                    .parts()
                    .iter()
                    .fold(date, |acc, (unit, value)| {
                        let value = if negate {
                            format!("-{value}")
                        } else {
                            value.to_string()
                        };
                        format!("DATEADD({unit}, {value}, {acc})")
                    })
            }
            // `PrestodbQuery.applyInterval`: one single-unit literal per part.
            Dialect::Presto | Dialect::Trino => split_sql_interval(interval)
                .iter()
                .map(|part| self.presto_interval_string(part))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .fold(date, |acc, part| format!("{acc} {op} interval {part}")),
            Dialect::Oracle => Self::oracle_apply_interval(date, interval, negate),
            Dialect::Pinot => self.pinot_apply_interval(date, interval, sign)?,
            Dialect::Hive
            | Dialect::Sqlite
            | Dialect::Druid
            | Dialect::Dremio
            | Dialect::Ksql
            | Dialect::QuestDb => self.apply_interval_parts(date, interval, negate)?,
            Dialect::Postgres
            | Dialect::CubeStore
            | Dialect::Crate
            | Dialect::Vertica
            | Dialect::Firebolt
            | Dialect::DuckDb => {
                format!(
                    "{date} {op} interval {}",
                    self.interval_string(interval.to_string())?
                )
            }
        })
    }

    /// `PostgresQuery.dateBin`: whole intervals, measured in epoch seconds.
    fn postgres_date_bin(interval_str: &str, source: &str, origin: &str) -> String {
        format!(
            "('{origin}'::timestamp + INTERVAL {interval_str} *\n      FLOOR(\n        EXTRACT(EPOCH FROM ({source} - '{origin}'::timestamp)) /\n        EXTRACT(EPOCH FROM INTERVAL {interval_str})\n      ))"
        )
    }

    /// `BigqueryQuery.applyInterval`: one date part at a time, coarsest first.
    fn bigquery_apply_interval(
        &self,
        direction: &str,
        date: String,
        interval: &str,
    ) -> Result<String, CubeError> {
        let mut acc = date;
        for part in split_sql_interval(interval) {
            let (formatted, time_unit) = Self::bigquery_interval(&part)?;
            acc = if matches!(time_unit.as_str(), "YEAR" | "MONTH" | "QUARTER")
                || formatted.contains("WEEK")
            {
                self.time_stamp_cast(format!(
                    "DATETIME_{direction}(DATETIME({acc}), INTERVAL {formatted})"
                ))?
            } else {
                format!("TIMESTAMP_{direction}({acc}, INTERVAL {formatted})")
            };
        }
        Ok(acc)
    }
}
