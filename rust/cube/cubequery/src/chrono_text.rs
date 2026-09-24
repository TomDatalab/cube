//! Port of the parts of `chrono-node` (v2, English "casual" configuration)
//! that `date-parser.js` relies on: free-text extraction of absolute dates
//! (`2020-02-02`, `Jan 5, 2020`, `5 January`, `1/5/2020`), casual references
//! (`now`, `today`, `yesterday`, `tomorrow`), relative expressions (`7 days
//! ago`, `2 weeks from now`, `in 3 hours`, `next 5 days`, `last month`),
//! weekdays and times of day, plus the `date + time` and `date to date`
//! merging refiners.
//!
//! All arithmetic is done on naive (wall-clock) datetimes: the Node.js code
//! feeds chrono a `Date` built from the wall-clock time in the query time
//! zone, so components come back as wall-clock values which are then
//! re-interpreted in that time zone by `date_parser`.

use std::sync::LazyLock;

use chrono::{Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use regex::{Captures, Regex};

use crate::moment::{add_months_clamped, js_weekday};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Second,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Meridiem {
    Am,
    Pm,
}

const YEAR: usize = 0;
const MONTH: usize = 1;
const DAY: usize = 2;
const HOUR: usize = 3;
const MINUTE: usize = 4;
const SECOND: usize = 5;
const MILLISECOND: usize = 6;

/// Port of chrono's `ParsingComponents`: every component has a value that is
/// either *known* (explicitly parsed) or *implied* (defaulted from the
/// reference date, or derived).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Components {
    values: [i64; 7],
    certain: [bool; 7],
    weekday: Option<i64>,
    weekday_certain: bool,
    meridiem: Option<Meridiem>,
    meridiem_certain: bool,
}

impl Components {
    fn new(reference: &NaiveDateTime) -> Self {
        Components {
            values: [
                reference.year() as i64,
                reference.month() as i64,
                reference.day() as i64,
                12,
                0,
                0,
                0,
            ],
            certain: [false; 7],
            weekday: None,
            weekday_certain: false,
            meridiem: None,
            meridiem_certain: false,
        }
    }

    fn get(&self, component: usize) -> i64 {
        self.values[component]
    }

    fn is_certain(&self, component: usize) -> bool {
        self.certain[component]
    }

    fn assign(&mut self, component: usize, value: i64) {
        self.values[component] = value;
        self.certain[component] = true;
    }

    fn imply(&mut self, component: usize, value: i64) {
        if !self.certain[component] {
            self.values[component] = value;
        }
    }

    fn assign_meridiem(&mut self, value: Meridiem) {
        self.meridiem = Some(value);
        self.meridiem_certain = true;
    }

    fn imply_meridiem(&mut self, value: Meridiem) {
        if !self.meridiem_certain {
            self.meridiem = Some(value);
        }
    }

    fn assign_weekday(&mut self, value: i64) {
        self.weekday = Some(value);
        self.weekday_certain = true;
    }

    fn imply_weekday(&mut self, value: i64) {
        if !self.weekday_certain {
            self.weekday = Some(value);
        }
    }

    fn assign_similar_date(&mut self, d: &NaiveDateTime) {
        self.assign(DAY, d.day() as i64);
        self.assign(MONTH, d.month() as i64);
        self.assign(YEAR, d.year() as i64);
    }

    fn imply_similar_date(&mut self, d: &NaiveDateTime) {
        self.imply(DAY, d.day() as i64);
        self.imply(MONTH, d.month() as i64);
        self.imply(YEAR, d.year() as i64);
    }

    fn assign_similar_time(&mut self, d: &NaiveDateTime) {
        self.assign(HOUR, d.hour() as i64);
        self.assign(MINUTE, d.minute() as i64);
        self.assign(SECOND, d.second() as i64);
        self.assign(MILLISECOND, (d.nanosecond() / 1_000_000) as i64);
        if self.get(HOUR) < 12 {
            self.assign_meridiem(Meridiem::Am);
        } else {
            self.assign_meridiem(Meridiem::Pm);
        }
    }

    fn imply_similar_time(&mut self, d: &NaiveDateTime) {
        self.imply(HOUR, d.hour() as i64);
        self.imply(MINUTE, d.minute() as i64);
        self.imply(SECOND, d.second() as i64);
        self.imply(MILLISECOND, (d.nanosecond() / 1_000_000) as i64);
    }

    fn is_only_date(&self) -> bool {
        !self.is_certain(HOUR) && !self.is_certain(MINUTE) && !self.is_certain(SECOND)
    }

    fn is_only_time(&self) -> bool {
        !self.weekday_certain && !self.is_certain(DAY) && !self.is_certain(MONTH)
    }

    fn is_only_weekday_component(&self) -> bool {
        self.weekday_certain && !self.is_certain(DAY) && !self.is_certain(MONTH)
    }

    /// The datetime the components describe, with JavaScript `Date` overflow
    /// semantics (month 13 rolls into the next year, hour 24 into the next
    /// day, ...). Used for comparisons while merging results.
    fn rolled_date(&self) -> NaiveDateTime {
        let year = self.get(YEAR).clamp(-262_000, 262_000) as i32;
        let base = NaiveDate::from_ymd_opt(year, 1, 1).expect("valid year");
        let date = add_months_clamped(base, self.get(MONTH) - 1);
        let midnight = NaiveTime::from_hms_opt(0, 0, 0).expect("midnight");
        date.and_time(midnight)
            + Duration::days(self.get(DAY) - 1)
            + Duration::hours(self.get(HOUR))
            + Duration::minutes(self.get(MINUTE))
            + Duration::seconds(self.get(SECOND))
            + Duration::milliseconds(self.get(MILLISECOND))
    }

    /// The datetime the components describe, or `None` when they do not form
    /// a real calendar date/time (chrono's `isValidDate`).
    pub(crate) fn to_naive(&self) -> Option<NaiveDateTime> {
        let year = i32::try_from(self.get(YEAR)).ok()?;
        let date = NaiveDate::from_ymd_opt(
            year,
            u32::try_from(self.get(MONTH)).ok()?,
            u32::try_from(self.get(DAY)).ok()?,
        )?;
        let time = NaiveTime::from_hms_milli_opt(
            u32::try_from(self.get(HOUR)).ok()?,
            u32::try_from(self.get(MINUTE)).ok()?,
            u32::try_from(self.get(SECOND)).ok()?,
            u32::try_from(self.get(MILLISECOND)).ok()?,
        )?;
        Some(date.and_time(time))
    }

    fn is_valid_date(&self) -> bool {
        self.to_naive().is_some()
    }
}

/// A date mention found in the text.
#[derive(Debug, Clone)]
pub(crate) struct ParsedResult {
    /// Byte index of the mention in the original text.
    pub index: usize,
    /// The matched text.
    pub text: String,
    pub start: Components,
    pub end: Option<Components>,
}

impl ParsedResult {
    fn end_index(&self) -> usize {
        self.index + self.text.len()
    }
}

struct Context<'a> {
    text: &'a str,
    reference: NaiveDateTime,
}

// ---------------------------------------------------------------------------
// Dictionaries and patterns (locales/en/constants.ts)
// ---------------------------------------------------------------------------

const WEEKDAYS: &[(&str, i64)] = &[
    ("sunday", 0),
    ("sun", 0),
    ("sun.", 0),
    ("monday", 1),
    ("mon", 1),
    ("mon.", 1),
    ("tuesday", 2),
    ("tue", 2),
    ("tue.", 2),
    ("wednesday", 3),
    ("wed", 3),
    ("wed.", 3),
    ("thursday", 4),
    ("thurs", 4),
    ("thurs.", 4),
    ("thur", 4),
    ("thur.", 4),
    ("thu", 4),
    ("thu.", 4),
    ("friday", 5),
    ("fri", 5),
    ("fri.", 5),
    ("saturday", 6),
    ("sat", 6),
    ("sat.", 6),
];

const FULL_MONTHS: &[(&str, i64)] = &[
    ("january", 1),
    ("february", 2),
    ("march", 3),
    ("april", 4),
    ("may", 5),
    ("june", 6),
    ("july", 7),
    ("august", 8),
    ("september", 9),
    ("october", 10),
    ("november", 11),
    ("december", 12),
];

const SHORT_MONTHS: &[(&str, i64)] = &[
    ("jan", 1),
    ("jan.", 1),
    ("feb", 2),
    ("feb.", 2),
    ("mar", 3),
    ("mar.", 3),
    ("apr", 4),
    ("apr.", 4),
    ("jun", 6),
    ("jun.", 6),
    ("jul", 7),
    ("jul.", 7),
    ("aug", 8),
    ("aug.", 8),
    ("sep", 9),
    ("sep.", 9),
    ("sept", 9),
    ("sept.", 9),
    ("oct", 10),
    ("oct.", 10),
    ("nov", 11),
    ("nov.", 11),
    ("dec", 12),
    ("dec.", 12),
];

const INTEGER_WORDS: &[(&str, f64)] = &[
    ("one", 1.0),
    ("two", 2.0),
    ("three", 3.0),
    ("four", 4.0),
    ("five", 5.0),
    ("six", 6.0),
    ("seven", 7.0),
    ("eight", 8.0),
    ("nine", 9.0),
    ("ten", 10.0),
    ("eleven", 11.0),
    ("twelve", 12.0),
];

const ORDINAL_WORDS: &[(&str, i64)] = &[
    ("first", 1),
    ("second", 2),
    ("third", 3),
    ("fourth", 4),
    ("fifth", 5),
    ("sixth", 6),
    ("seventh", 7),
    ("eighth", 8),
    ("ninth", 9),
    ("tenth", 10),
    ("eleventh", 11),
    ("twelfth", 12),
    ("thirteenth", 13),
    ("fourteenth", 14),
    ("fifteenth", 15),
    ("sixteenth", 16),
    ("seventeenth", 17),
    ("eighteenth", 18),
    ("nineteenth", 19),
    ("twentieth", 20),
    ("twenty first", 21),
    ("twenty-first", 21),
    ("twenty second", 22),
    ("twenty-second", 22),
    ("twenty third", 23),
    ("twenty-third", 23),
    ("twenty fourth", 24),
    ("twenty-fourth", 24),
    ("twenty fifth", 25),
    ("twenty-fifth", 25),
    ("twenty sixth", 26),
    ("twenty-sixth", 26),
    ("twenty seventh", 27),
    ("twenty-seventh", 27),
    ("twenty eighth", 28),
    ("twenty-eighth", 28),
    ("twenty ninth", 29),
    ("twenty-ninth", 29),
    ("thirtieth", 30),
    ("thirty first", 31),
    ("thirty-first", 31),
];

const TIME_UNITS: &[(&str, Unit)] = &[
    ("s", Unit::Second),
    ("sec", Unit::Second),
    ("second", Unit::Second),
    ("seconds", Unit::Second),
    ("m", Unit::Minute),
    ("min", Unit::Minute),
    ("mins", Unit::Minute),
    ("minute", Unit::Minute),
    ("minutes", Unit::Minute),
    ("h", Unit::Hour),
    ("hr", Unit::Hour),
    ("hrs", Unit::Hour),
    ("hour", Unit::Hour),
    ("hours", Unit::Hour),
    ("d", Unit::Day),
    ("day", Unit::Day),
    ("days", Unit::Day),
    ("w", Unit::Week),
    ("week", Unit::Week),
    ("weeks", Unit::Week),
    ("mo", Unit::Month),
    ("mon", Unit::Month),
    ("mos", Unit::Month),
    ("month", Unit::Month),
    ("months", Unit::Month),
    ("qtr", Unit::Quarter),
    ("quarter", Unit::Quarter),
    ("quarters", Unit::Quarter),
    ("y", Unit::Year),
    ("yr", Unit::Year),
    ("year", Unit::Year),
    ("years", Unit::Year),
];

/// `matchAnyPattern`: alternation of the terms, longest first.
fn match_any(terms: impl Iterator<Item = &'static str>) -> String {
    let mut terms: Vec<&str> = terms.collect();
    terms.sort_by_key(|t| std::cmp::Reverse(t.len()));
    let joined = terms
        .iter()
        .map(|t| regex::escape(t))
        .collect::<Vec<_>>()
        .join("|");
    format!("(?:{joined})")
}

fn month_pattern() -> String {
    match_any(FULL_MONTHS.iter().chain(SHORT_MONTHS).map(|(k, _)| *k))
}

fn lookup_month(name: &str) -> Option<i64> {
    let lower = name.to_ascii_lowercase();
    FULL_MONTHS
        .iter()
        .chain(SHORT_MONTHS)
        .find(|(k, _)| *k == lower)
        .map(|(_, v)| *v)
}

fn is_full_month(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    FULL_MONTHS.iter().any(|(k, _)| *k == lower)
}

fn lookup_weekday(name: &str) -> Option<i64> {
    let lower = name.to_ascii_lowercase();
    WEEKDAYS.iter().find(|(k, _)| *k == lower).map(|(_, v)| *v)
}

fn lookup_unit(name: &str) -> Option<Unit> {
    let lower = name.to_ascii_lowercase();
    TIME_UNITS
        .iter()
        .find(|(k, _)| *k == lower)
        .map(|(_, v)| *v)
}

fn number_pattern() -> String {
    let words = match_any(INTEGER_WORDS.iter().map(|(k, _)| *k));
    format!(
        "(?:{words}|[0-9]+|[0-9]+\\.[0-9]+|half(?:\\s{{0,2}}an?)?|an?\\b(?:\\s{{0,2}}few)?|few|several|the|a?\\s{{0,2}}couple\\s{{0,2}}(?:of)?)"
    )
}

fn parse_number(text: &str) -> f64 {
    let num = text.to_ascii_lowercase();
    if let Some((_, v)) = INTEGER_WORDS.iter().find(|(k, _)| *k == num) {
        return *v;
    }
    if num == "a" || num == "an" || num == "the" {
        return 1.0;
    }
    if num.contains("few") {
        return 3.0;
    }
    if num.contains("half") {
        return 0.5;
    }
    if num.contains("couple") {
        return 2.0;
    }
    if num.contains("several") {
        return 7.0;
    }
    num.trim().parse().unwrap_or(f64::NAN)
}

fn ordinal_pattern() -> String {
    let words = match_any(ORDINAL_WORDS.iter().map(|(k, _)| *k));
    format!("(?:{words}|[0-9]{{1,2}}(?:st|nd|rd|th)?)")
}

fn parse_ordinal(text: &str) -> i64 {
    let num = text.to_ascii_lowercase();
    if let Some((_, v)) = ORDINAL_WORDS.iter().find(|(k, _)| *k == num) {
        return *v;
    }
    let digits: String = num.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().unwrap_or(0)
}

const YEAR_PATTERN: &str =
    "(?:[1-9][0-9]{0,3}\\s{0,2}(?:BE|AD|BC|BCE|CE)|[1-2][0-9]{3}|[5-9][0-9])";

fn find_most_likely_ad_year(year: i64) -> i64 {
    if year < 100 {
        if year > 50 {
            year + 1900
        } else {
            year + 2000
        }
    } else {
        year
    }
}

fn parse_year(text: &str) -> i64 {
    let upper = text.to_ascii_uppercase();
    let digits = |s: &str| -> i64 {
        s.chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap_or(0)
    };
    if upper.contains("BE") && !upper.contains("BCE") {
        return digits(&upper) - 543;
    }
    if upper.contains("BC") {
        return -digits(&upper);
    }
    if upper.contains("AD") || upper.contains("CE") {
        return digits(&upper);
    }
    find_most_likely_ad_year(digits(&upper))
}

fn single_time_unit_pattern() -> String {
    format!(
        "({})\\s{{0,3}}({})",
        number_pattern(),
        match_any(TIME_UNITS.iter().map(|(k, _)| *k))
    )
}

/// `TIME_UNITS_PATTERN`: up to eleven `<number> <unit>` fragments.
fn time_units_pattern() -> String {
    let single = single_time_unit_pattern();
    // Turn the capturing groups of the single pattern into non-capturing ones.
    // `(?:` is parked on a sentinel so the second pass can turn both the
    // capturing groups and the parked ones into `(?:` in one go.
    let no_capture = single
        .replace("(?:", "\u{0}")
        .replace(['(', '\u{0}'], "(?:");
    format!(
        "(?:(?:about|around)\\s{{0,3}})?{no_capture}\\s{{0,5}}(?:,?\\s{{0,5}}{no_capture}){{0,10}}"
    )
}

static SINGLE_TIME_UNIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("(?i){}", single_time_unit_pattern())).expect("regex"));

fn parse_time_units(text: &str) -> Vec<(Unit, f64)> {
    let mut fragments: Vec<(Unit, f64)> = Vec::new();
    let mut remaining = text;
    while let Some(caps) = SINGLE_TIME_UNIT_RE.captures(remaining) {
        let amount = parse_number(&caps[1]);
        if let Some(unit) = lookup_unit(&caps[2]) {
            fragments.retain(|(u, _)| *u != unit);
            fragments.push((unit, amount));
        }
        let end = caps.get(0).map(|m| m.end()).unwrap_or(remaining.len());
        remaining = remaining[end..].trim();
    }
    fragments
}

fn reverse_time_units(units: &[(Unit, f64)]) -> Vec<(Unit, f64)> {
    units.iter().map(|(u, n)| (*u, -*n)).collect()
}

// ---------------------------------------------------------------------------
// dayjs arithmetic on naive datetimes
// ---------------------------------------------------------------------------

fn dayjs_add(date: NaiveDateTime, amount: f64, unit: Unit) -> NaiveDateTime {
    if !amount.is_finite() {
        return date;
    }
    match unit {
        Unit::Second => date + Duration::milliseconds((amount * 1000.0) as i64),
        Unit::Minute => date + Duration::milliseconds((amount * 60_000.0) as i64),
        Unit::Hour => date + Duration::milliseconds((amount * 3_600_000.0) as i64),
        Unit::Day => date + Duration::days(amount.round() as i64),
        Unit::Week => date + Duration::days((amount * 7.0).round() as i64),
        Unit::Month => add_months_clamped(date.date(), amount.trunc() as i64).and_time(date.time()),
        Unit::Quarter => {
            add_months_clamped(date.date(), (amount * 3.0).trunc() as i64).and_time(date.time())
        }
        Unit::Year => {
            add_months_clamped(date.date(), amount.trunc() as i64 * 12).and_time(date.time())
        }
    }
}

fn create_relative_from_reference(reference: &NaiveDateTime, units: &[(Unit, f64)]) -> Components {
    let mut date = *reference;
    for (unit, amount) in units {
        date = dayjs_add(date, *amount, *unit);
    }
    let has = |unit: Unit| units.iter().any(|(u, n)| *u == unit && *n != 0.0);

    let mut components = Components::new(reference);
    if has(Unit::Hour) || has(Unit::Minute) || has(Unit::Second) {
        components.assign_similar_time(&date);
        components.assign_similar_date(&date);
    } else {
        components.imply_similar_time(&date);
        if has(Unit::Day) {
            components.assign_similar_date(&date);
        } else {
            if has(Unit::Week) {
                components.imply_weekday(js_weekday(date.weekday()));
            }
            components.imply(DAY, date.day() as i64);
            if has(Unit::Month) {
                components.assign(MONTH, date.month() as i64);
                components.assign(YEAR, date.year() as i64);
            } else {
                components.imply(MONTH, date.month() as i64);
                if has(Unit::Year) {
                    components.assign(YEAR, date.year() as i64);
                } else {
                    components.imply(YEAR, date.year() as i64);
                }
            }
        }
    }
    components
}

fn find_year_closest_to_ref(reference: &NaiveDateTime, day: i64, month: i64) -> i64 {
    // dayjs: `ref.month(month - 1)` clamps the day of month, `.date(day)`
    // rolls over, `.year(ref.year)` rolls Feb 29 into Mar 1.
    let with_month = add_months_clamped(reference.date(), month - reference.month() as i64);
    let with_day = with_month.with_day(1).expect("first of month") + Duration::days(day - 1);
    let base = with_day
        .with_year(reference.year())
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(reference.year(), 3, 1).expect("march first"))
        .and_time(reference.time());
    let diff = |d: NaiveDateTime| (d - *reference).num_milliseconds().abs();
    let next_year = add_months_clamped(base.date(), 12).and_time(base.time());
    let last_year = add_months_clamped(base.date(), -12).and_time(base.time());
    if diff(next_year) < diff(base) {
        next_year.year() as i64
    } else if diff(last_year) < diff(base) {
        last_year.year() as i64
    } else {
        base.year() as i64
    }
}

// ---------------------------------------------------------------------------
// Parser runner
// ---------------------------------------------------------------------------

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `(?=\W|$)` at `end`.
fn boundary_after(text: &str, end: usize) -> bool {
    text[end..].chars().next().is_none_or(|c| !is_word_char(c))
}

/// Emulates `(?=\W|$)` with the backtracking a JavaScript regex would do over
/// trailing whitespace: returns the largest `end` in `min_end..=end` at which
/// the boundary holds, giving up as soon as a non-whitespace char is hit.
fn boundary_with_backtracking(text: &str, end: usize, min_end: usize) -> Option<usize> {
    let mut e = end;
    loop {
        if boundary_after(text, e) {
            return Some(e);
        }
        if e <= min_end {
            return None;
        }
        let last = text[..e].chars().next_back()?;
        if !last.is_whitespace() {
            return None;
        }
        e -= last.len_utf8();
    }
}

type Extract = dyn Fn(&Context, &Captures, usize, &str) -> Option<ParsedResult>;

/// Port of `Chrono.executeParser` for parsers whose first capture group is
/// the left word boundary (`AbstractParserWithWordBoundaryChecking`).
///
/// `extract` receives the context, the captures, the absolute index of the
/// inner match and the inner matched text (after the trailing boundary has
/// been applied when `boundary` is set).
fn run_parser(ctx: &Context, re: &Regex, boundary: bool, extract: &Extract) -> Vec<ParsedResult> {
    let mut results = Vec::new();
    let mut offset = 0;
    while offset <= ctx.text.len() {
        let Some(remaining) = ctx.text.get(offset..) else {
            break;
        };
        let Some(caps) = re.captures(remaining) else {
            break;
        };
        let whole = caps.get(0).expect("match");
        let header_len = caps.get(1).map(|m| m.len()).unwrap_or(0);
        let inner_start = whole.start() + header_len;
        let abs_index = offset + inner_start;
        let mut inner_end = whole.end();
        if boundary {
            match boundary_with_backtracking(remaining, inner_end, inner_start) {
                Some(e) => inner_end = e,
                None => {
                    offset = abs_index + 1;
                    continue;
                }
            }
        }
        let inner_text = &remaining[inner_start..inner_end];
        match extract(ctx, &caps, abs_index, inner_text) {
            Some(result) => {
                offset = result.end_index().max(abs_index + 1);
                results.push(result);
            }
            None => offset = abs_index + 1,
        }
    }
    results
}

fn result_at(index: usize, text: &str, start: Components) -> ParsedResult {
    ParsedResult {
        index,
        text: text.to_string(),
        start,
        end: None,
    }
}

fn group<'a>(caps: &'a Captures, i: usize) -> Option<&'a str> {
    caps.get(i).map(|m| m.as_str())
}

fn parse_int(text: &str) -> i64 {
    text.trim().parse().unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

static ISO_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        "(?i)(\\W|^)([0-9]{4})-([0-9]{1,2})-([0-9]{1,2})(?:T([0-9]{1,2}):([0-9]{1,2})(?::([0-9]{1,2})(?:\\.(\\d{1,4}))?)?(?:Z|([+-]\\d{2}):?(\\d{2})?)?)?",
    )
    .expect("regex")
});

fn iso_format(ctx: &Context) -> Vec<ParsedResult> {
    run_parser(ctx, &ISO_RE, true, &|ctx, caps, index, text| {
        let mut c = Components::new(&ctx.reference);
        c.assign(YEAR, parse_int(&caps[2]));
        c.assign(MONTH, parse_int(&caps[3]));
        c.assign(DAY, parse_int(&caps[4]));
        if let Some(hour) = group(caps, 5) {
            c.assign(HOUR, parse_int(hour));
            c.assign(MINUTE, parse_int(group(caps, 6).unwrap_or("0")));
            if let Some(second) = group(caps, 7) {
                c.assign(SECOND, parse_int(second));
            }
            if let Some(ms) = group(caps, 8) {
                c.assign(MILLISECOND, parse_int(ms));
            }
        }
        Some(result_at(index, text, c))
    })
}

static CASUAL_RELATIVE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        "(?i)(\\W|^)(this|last|past|next|after|\\+|-)\\s*({})",
        time_units_pattern()
    ))
    .expect("regex")
});

fn time_unit_casual_relative(ctx: &Context) -> Vec<ParsedResult> {
    run_parser(ctx, &CASUAL_RELATIVE_RE, true, &|ctx, caps, index, text| {
        let prefix = caps[2].to_ascii_lowercase();
        let mut units = parse_time_units(&caps[3]);
        if matches!(prefix.as_str(), "last" | "past" | "-") {
            units = reverse_time_units(&units);
        }
        Some(result_at(
            index,
            text,
            create_relative_from_reference(&ctx.reference, &units),
        ))
    })
}

static RELATIVE_DATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        "(?i)(\\W|^)(this|last|past|next|after\\s*this)\\s*({})",
        match_any(TIME_UNITS.iter().map(|(k, _)| *k))
    ))
    .expect("regex")
});

fn relative_date_format(ctx: &Context) -> Vec<ParsedResult> {
    run_parser(ctx, &RELATIVE_DATE_RE, true, &|ctx, caps, index, text| {
        let modifier = caps[2].to_ascii_lowercase();
        let unit_word = caps[3].to_ascii_lowercase();
        let unit = lookup_unit(&unit_word)?;
        if modifier == "next" || modifier.starts_with("after") {
            let c = create_relative_from_reference(&ctx.reference, &[(unit, 1.0)]);
            return Some(result_at(index, text, c));
        }
        if modifier == "last" || modifier == "past" {
            let c = create_relative_from_reference(&ctx.reference, &[(unit, -1.0)]);
            return Some(result_at(index, text, c));
        }
        let mut c = Components::new(&ctx.reference);
        let reference = ctx.reference;
        if unit_word.contains("week") {
            let date = reference - Duration::days(js_weekday(reference.weekday()));
            c.imply(DAY, date.day() as i64);
            c.imply(MONTH, date.month() as i64);
            c.imply(YEAR, date.year() as i64);
        } else if unit_word.contains("month") {
            let date = reference - Duration::days(reference.day() as i64 - 1);
            c.imply(DAY, date.day() as i64);
            c.assign(YEAR, date.year() as i64);
            c.assign(MONTH, date.month() as i64);
        } else if unit_word.contains("year") {
            let date = reference - Duration::days(reference.day() as i64 - 1);
            let date =
                add_months_clamped(date.date(), -(date.month0() as i64)).and_time(date.time());
            c.imply(DAY, date.day() as i64);
            c.imply(MONTH, date.month() as i64);
            c.assign(YEAR, date.year() as i64);
        }
        Some(result_at(index, text, c))
    })
}

static MONTH_NAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        "(?i)(\\W|^)((?:in)\\s*)?({})\\s*(?:[,-]?\\s*({YEAR_PATTERN}))?",
        month_pattern()
    ))
    .expect("regex")
});

/// `(?=[^\s\w]|\s+[^0-9]|\s+$|$)` of `ENMonthNameParser`.
fn month_name_lookahead(text: &str, end: usize) -> bool {
    let rest = &text[end..];
    let mut chars = rest.chars();
    match chars.next() {
        None => true,
        Some(c) if !c.is_whitespace() && !is_word_char(c) => true,
        Some(c) if c.is_whitespace() => {
            let after_ws = rest.trim_start();
            after_ws.is_empty() || !after_ws.starts_with(|c: char| c.is_ascii_digit())
        }
        _ => false,
    }
}

fn month_name(ctx: &Context) -> Vec<ParsedResult> {
    // The lookahead may reject the greedy match; emulate the regex engine's
    // backtracking by retrying with the shorter forms (without year, then
    // with less trailing whitespace).
    let mut results = Vec::new();
    let mut offset = 0;
    while offset <= ctx.text.len() {
        let Some(remaining) = ctx.text.get(offset..) else {
            break;
        };
        let Some(caps) = MONTH_NAME_RE.captures(remaining) else {
            break;
        };
        let whole = caps.get(0).expect("match");
        let prefix_len = caps.get(2).map(|m| m.len()).unwrap_or(0);
        let header_len = caps.get(1).map(|m| m.len()).unwrap_or(0);
        let month_match = caps.get(3).expect("month");
        let abs_index = offset + whole.start() + header_len;

        let mut candidates = vec![whole.end()];
        if caps.get(4).is_some() {
            candidates.push(month_match.end());
        }
        let mut accepted: Option<(usize, bool)> = None;
        for candidate in candidates {
            let with_year = candidate == whole.end() && caps.get(4).is_some();
            let mut e = candidate;
            loop {
                if month_name_lookahead(remaining, e) {
                    accepted = Some((e, with_year));
                    break;
                }
                if e <= month_match.end() {
                    break;
                }
                let last = remaining[..e].chars().next_back().expect("char");
                e -= last.len_utf8();
            }
            if accepted.is_some() {
                break;
            }
        }
        let Some((end, with_year)) = accepted else {
            offset = abs_index + 1;
            continue;
        };
        let full_len = end - whole.start();
        let month_name = month_match.as_str();
        if full_len <= 3 && !is_full_month(month_name) {
            offset = abs_index + 1;
            continue;
        }
        let Some(month) = lookup_month(month_name) else {
            offset = abs_index + 1;
            continue;
        };
        let mut c = Components::new(&ctx.reference);
        c.imply(DAY, 1);
        c.assign(MONTH, month);
        if with_year {
            c.assign(YEAR, parse_year(&caps[4]));
        } else {
            c.imply(YEAR, find_year_closest_to_ref(&ctx.reference, 1, month));
        }
        let index = abs_index + prefix_len;
        let text = &remaining[whole.start() + header_len + prefix_len..end];
        let result = result_at(index, text, c);
        offset = result.end_index().max(abs_index + 1);
        results.push(result);
    }
    results
}

static CASUAL_TIME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("(?i)(\\W|^)(?:this)?\\s{0,3}(morning|afternoon|evening|night|midnight|midday|noon)")
        .expect("regex")
});

fn casual_time(ctx: &Context) -> Vec<ParsedResult> {
    run_parser(ctx, &CASUAL_TIME_RE, true, &|ctx, caps, index, text| {
        let mut c = Components::new(&ctx.reference);
        let imply_zero_rest = |c: &mut Components| {
            c.imply(MINUTE, 0);
            c.imply(SECOND, 0);
            c.imply(MILLISECOND, 0);
        };
        match caps[2].to_ascii_lowercase().as_str() {
            "afternoon" => {
                c.imply_meridiem(Meridiem::Pm);
                c.imply(HOUR, 15);
                imply_zero_rest(&mut c);
            }
            "evening" | "night" => {
                c.imply_meridiem(Meridiem::Pm);
                c.imply(HOUR, 20);
                imply_zero_rest(&mut c);
            }
            "midnight" => {
                if ctx.reference.hour() > 2 {
                    let next = ctx.reference + Duration::days(1);
                    c.imply_similar_date(&next);
                    c.imply_similar_time(&next);
                }
                c.assign(HOUR, 0);
                imply_zero_rest(&mut c);
            }
            "morning" => {
                c.imply_meridiem(Meridiem::Am);
                c.imply(HOUR, 6);
                imply_zero_rest(&mut c);
            }
            "noon" | "midday" => {
                c.imply_meridiem(Meridiem::Am);
                c.imply(HOUR, 12);
                imply_zero_rest(&mut c);
            }
            _ => return None,
        }
        Some(result_at(index, text, c))
    })
}

static CASUAL_DATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("(?i)(\\W|^)(now|today|tonight|tomorrow|tmr|tmrw|yesterday|last\\s*night)")
        .expect("regex")
});

fn casual_date(ctx: &Context) -> Vec<ParsedResult> {
    run_parser(ctx, &CASUAL_DATE_RE, true, &|ctx, caps, index, text| {
        let reference = ctx.reference;
        let mut c = Components::new(&reference);
        match caps[2].to_ascii_lowercase().as_str() {
            "now" => {
                c.assign_similar_date(&reference);
                c.assign_similar_time(&reference);
            }
            "today" => {
                c.assign_similar_date(&reference);
                c.imply_similar_time(&reference);
            }
            "yesterday" => {
                let target = reference - Duration::days(1);
                c.assign_similar_date(&target);
                c.imply_similar_time(&target);
            }
            "tomorrow" | "tmr" | "tmrw" => {
                let target = reference + Duration::days(1);
                c.assign_similar_date(&target);
                c.imply_similar_time(&target);
            }
            "tonight" => {
                c.imply(HOUR, 22);
                c.imply_meridiem(Meridiem::Pm);
                c.assign_similar_date(&reference);
            }
            _ => {
                // last night
                let mut target = reference;
                if target.hour() > 6 {
                    target -= Duration::days(1);
                }
                c.assign_similar_date(&target);
                c.imply(HOUR, 0);
            }
        }
        Some(result_at(index, text, c))
    })
}

static SLASH_DATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        "(?i)([^\\d]|^)([0-3]?[0-9])[/.\\-]([0-3]?[0-9])(?:[/.\\-]([0-9]{4}|[0-9]{2}))?(\\W|$)",
    )
    .expect("regex")
});
static SLASH_DATE_UNLIKELY_1: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("^\\d\\.\\d$").expect("regex"));
static SLASH_DATE_UNLIKELY_2: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("^\\d\\.\\d{1,2}\\.\\d{1,2}\\s*$").expect("regex"));

fn slash_date(ctx: &Context) -> Vec<ParsedResult> {
    let mut results = Vec::new();
    let mut offset = 0;
    while offset <= ctx.text.len() {
        let Some(remaining) = ctx.text.get(offset..) else {
            break;
        };
        let Some(caps) = SLASH_DATE_RE.captures(remaining) else {
            break;
        };
        let whole = caps.get(0).expect("match");
        let opening_len = caps.get(1).map(|m| m.len()).unwrap_or(0);
        let ending_len = caps.get(5).map(|m| m.len()).unwrap_or(0);
        let inner_start = whole.start() + opening_len;
        let abs_index = offset + inner_start;

        let extracted = (|| {
            if opening_len == 0 && whole.start() > 0 && whole.start() < remaining.len() {
                let prev = remaining[..whole.start()].chars().next_back();
                if prev.is_some_and(|c| c.is_ascii_digit()) {
                    return None;
                }
            }
            let text = &remaining[inner_start..whole.end() - ending_len];
            if SLASH_DATE_UNLIKELY_1.is_match(text) || SLASH_DATE_UNLIKELY_2.is_match(text) {
                return None;
            }
            if caps.get(4).is_none() && !whole.as_str().contains('/') {
                return None;
            }
            let mut month = parse_int(&caps[2]);
            let mut day = parse_int(&caps[3]);
            if !(1..=12).contains(&month) {
                if month > 12 && (1..=12).contains(&day) && month <= 31 {
                    std::mem::swap(&mut day, &mut month);
                } else {
                    return None;
                }
            }
            if !(1..=31).contains(&day) {
                return None;
            }
            let mut c = Components::new(&ctx.reference);
            c.assign(DAY, day);
            c.assign(MONTH, month);
            if let Some(year) = group(&caps, 4) {
                c.assign(YEAR, find_most_likely_ad_year(parse_int(year)));
            } else {
                c.imply(YEAR, find_year_closest_to_ref(&ctx.reference, day, month));
            }
            Some(result_at(abs_index, text, c))
        })();

        match extracted {
            Some(result) => {
                offset = result.end_index().max(abs_index + 1);
                results.push(result);
            }
            None => offset = abs_index + 1,
        }
    }
    results
}

static WITHIN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        "(?i)(\\W|^)(?:within|in|for)\\s*(?:(?:about|around|roughly|approximately|just)\\s*(?:~\\s*)?)?({})",
        time_units_pattern()
    ))
    .expect("regex")
});

fn time_unit_within(ctx: &Context) -> Vec<ParsedResult> {
    run_parser(ctx, &WITHIN_RE, true, &|ctx, caps, index, text| {
        let units = parse_time_units(&caps[2]);
        Some(result_at(
            index,
            text,
            create_relative_from_reference(&ctx.reference, &units),
        ))
    })
}

static LITTLE_ENDIAN_RE: LazyLock<Regex> = LazyLock::new(|| {
    let ordinal = ordinal_pattern();
    Regex::new(&format!(
        "(?i)(\\W|^)(?:on\\s{{0,3}})?({ordinal})(?:\\s{{0,3}}(?:to|-|–|until|through|till)?\\s{{0,3}}({ordinal}))?(?:-|/|\\s{{0,3}}(?:of)?\\s{{0,3}})({})(?:(?:-|/|,?\\s{{0,3}})({YEAR_PATTERN}))?",
        month_pattern()
    ))
    .expect("regex")
});

fn month_name_little_endian(ctx: &Context) -> Vec<ParsedResult> {
    run_parser(ctx, &LITTLE_ENDIAN_RE, true, &|ctx, caps, index, text| {
        let month = lookup_month(&caps[4])?;
        let day = parse_ordinal(&caps[2]);
        if day > 31 {
            return None;
        }
        let mut c = Components::new(&ctx.reference);
        c.assign(MONTH, month);
        c.assign(DAY, day);
        if let Some(year) = group(caps, 5) {
            c.assign(YEAR, parse_year(year));
        } else {
            c.imply(YEAR, find_year_closest_to_ref(&ctx.reference, day, month));
        }
        let mut result = result_at(index, text, c);
        if let Some(to) = group(caps, 3) {
            let mut end = result.start.clone();
            end.assign(DAY, parse_ordinal(to));
            result.end = Some(end);
        }
        Some(result)
    })
}

static MIDDLE_ENDIAN_RE: LazyLock<Regex> = LazyLock::new(|| {
    let ordinal = ordinal_pattern();
    Regex::new(&format!(
        "(?i)(\\W|^)({})(?:-|/|\\s*,?\\s*)({ordinal})\\s*(?:(?:to|-)\\s*({ordinal})\\s*)?(?:(?:-|/|\\s*,?\\s*)({YEAR_PATTERN}))?",
        month_pattern()
    ))
    .expect("regex")
});
static COLON_DIGIT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new("^:\\d").expect("regex"));

fn month_name_middle_endian(ctx: &Context) -> Vec<ParsedResult> {
    run_parser(ctx, &MIDDLE_ENDIAN_RE, true, &|ctx, caps, index, text| {
        // `(?!\:\d)` after the match.
        if COLON_DIGIT_RE.is_match(&ctx.text[index + text.len()..]) {
            return None;
        }
        let month = lookup_month(&caps[2])?;
        let day = parse_ordinal(&caps[3]);
        if day > 31 {
            return None;
        }
        let mut c = Components::new(&ctx.reference);
        c.assign(DAY, day);
        c.assign(MONTH, month);
        if let Some(year) = group(caps, 5) {
            c.assign(YEAR, parse_year(year));
        } else {
            c.imply(YEAR, find_year_closest_to_ref(&ctx.reference, day, month));
        }
        let mut result = result_at(index, text, c);
        if let Some(to) = group(caps, 4) {
            let mut end = result.start.clone();
            end.assign(DAY, parse_ordinal(to));
            result.end = Some(end);
        }
        Some(result)
    })
}

static WEEKDAY_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        "(?i)(\\W|^)(?:(?:,|\\(|（)\\s*)?(?:on\\s*?)?(?:(this|last|past|next)\\s*)?({})(?:\\s*(?:,|\\)|）))?(?:\\s*(this|last|past|next)\\s*week)?",
        match_any(WEEKDAYS.iter().map(|(k, _)| *k))
    ))
    .expect("regex")
});

fn days_forward_to_weekday(ref_weekday: i64, weekday: i64) -> i64 {
    let mut forward = weekday - ref_weekday;
    if forward < 0 {
        forward += 7;
    }
    forward
}

fn days_backward_to_weekday(ref_weekday: i64, weekday: i64) -> i64 {
    let mut backward = weekday - ref_weekday;
    if backward >= 0 {
        backward -= 7;
    }
    backward
}

fn days_to_weekday(ref_weekday: i64, weekday: i64, modifier: Option<&str>) -> i64 {
    match modifier {
        Some("this") => days_forward_to_weekday(ref_weekday, weekday),
        Some("last") => days_backward_to_weekday(ref_weekday, weekday),
        Some("next") => {
            if ref_weekday == 0 {
                return if weekday == 0 { 7 } else { weekday };
            }
            if ref_weekday == 6 {
                return match weekday {
                    6 => 7,
                    0 => 8,
                    _ => 1 + weekday,
                };
            }
            if weekday < ref_weekday && weekday != 0 {
                days_forward_to_weekday(ref_weekday, weekday)
            } else {
                days_forward_to_weekday(ref_weekday, weekday) + 7
            }
        }
        _ => {
            let backward = days_backward_to_weekday(ref_weekday, weekday);
            let forward = days_forward_to_weekday(ref_weekday, weekday);
            if forward < -backward {
                forward
            } else {
                backward
            }
        }
    }
}

fn weekday(ctx: &Context) -> Vec<ParsedResult> {
    run_parser(ctx, &WEEKDAY_RE, true, &|ctx, caps, index, text| {
        let weekday = lookup_weekday(&caps[3])?;
        let modifier_word = group(caps, 2)
            .or_else(|| group(caps, 4))
            .unwrap_or("")
            .to_ascii_lowercase();
        let modifier = match modifier_word.as_str() {
            "last" | "past" => Some("last"),
            "next" => Some("next"),
            "this" => Some("this"),
            _ => None,
        };
        let ref_weekday = js_weekday(ctx.reference.weekday());
        let days = days_to_weekday(ref_weekday, weekday, modifier);
        let mut c = Components::new(&ctx.reference);
        let date = c.rolled_date() + Duration::days(days);
        c.imply(DAY, date.day() as i64);
        c.imply(MONTH, date.month() as i64);
        c.imply(YEAR, date.year() as i64);
        c.assign_weekday(weekday);
        Some(result_at(index, text, c))
    })
}

static CASUAL_YMD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        "(?i)(\\W|^)([0-9]{{4}})[./\\s](?:({})|([0-9]{{1,2}}))[./\\s]([0-9]{{1,2}})",
        month_pattern()
    ))
    .expect("regex")
});

fn casual_year_month_day(ctx: &Context) -> Vec<ParsedResult> {
    run_parser(ctx, &CASUAL_YMD_RE, true, &|ctx, caps, index, text| {
        let month = match group(caps, 4) {
            Some(number) => parse_int(number),
            None => lookup_month(&caps[3])?,
        };
        if !(1..=12).contains(&month) {
            return None;
        }
        let mut c = Components::new(&ctx.reference);
        c.assign(DAY, parse_int(&caps[5]));
        c.assign(MONTH, month);
        c.assign(YEAR, parse_int(&caps[2]));
        Some(result_at(index, text, c))
    })
}

static SLASH_MONTH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("(?i)(\\W|^)([0-9]|0[1-9]|1[012])/([0-9]{4})").expect("regex"));

fn slash_month(ctx: &Context) -> Vec<ParsedResult> {
    run_parser(ctx, &SLASH_MONTH_RE, false, &|ctx, caps, index, text| {
        let mut c = Components::new(&ctx.reference);
        c.imply(DAY, 1);
        c.assign(MONTH, parse_int(&caps[2]));
        c.assign(YEAR, parse_int(&caps[3]));
        Some(result_at(index, text, c))
    })
}

static TIME_EXPRESSION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        "(?i)(^|\\s|T|\\b)(?:(?:at|from)\\s*)??(\\d{1,4})(?:(?:\\.|:|：)(\\d{1,2})(?:(?::|：)(\\d{2})(?:\\.(\\d{1,6}))?)?)?(?:\\s*(a\\.m\\.|p\\.m\\.|am?|pm?))?(?:\\s*(?:o\\W*clock|at\\s*night|in\\s*the\\s*(?:morning|afternoon)))?",
    )
    .expect("regex")
});
static SINGLE_DIGIT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new("^\\d$").expect("regex"));
static THREE_PLUS_DIGITS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("^\\d\\d\\d+$").expect("regex"));
static DIGIT_AP_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new("(?i)\\d[ap]$").expect("regex"));
static ENDING_NUMBERS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("[^\\d:.](\\d[\\d.]+)$").expect("regex"));
static DECIMAL_PAIRS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("\\d(\\.\\d{2})+$").expect("regex"));

fn time_expression(ctx: &Context) -> Vec<ParsedResult> {
    run_parser(ctx, &TIME_EXPRESSION_RE, true, &|ctx, caps, index, text| {
        // `(?!/)` after the match.
        if ctx.text[index + text.len()..].starts_with('/') {
            return None;
        }
        let mut c = Components::new(&ctx.reference);
        let mut minute = 0;
        let mut meridiem: Option<Meridiem> = None;
        let mut hour = parse_int(&caps[2]);
        if hour > 100 {
            if group(caps, 3).is_some() {
                return None;
            }
            minute = hour % 100;
            hour /= 100;
        }
        if hour > 24 {
            return None;
        }
        if let Some(m) = group(caps, 3) {
            if m.len() == 1 && group(caps, 6).is_none() {
                return None;
            }
            minute = parse_int(m);
        }
        if minute >= 60 {
            return None;
        }
        if hour > 12 {
            meridiem = Some(Meridiem::Pm);
        }
        if let Some(ampm) = group(caps, 6) {
            if hour > 12 {
                return None;
            }
            match ampm.chars().next().map(|c| c.to_ascii_lowercase()) {
                Some('a') => {
                    meridiem = Some(Meridiem::Am);
                    if hour == 12 {
                        hour = 0;
                    }
                }
                Some('p') => {
                    meridiem = Some(Meridiem::Pm);
                    if hour != 12 {
                        hour += 12;
                    }
                }
                _ => {}
            }
        }
        c.assign(HOUR, hour);
        c.assign(MINUTE, minute);
        match meridiem {
            Some(m) => c.assign_meridiem(m),
            None => c.imply_meridiem(if hour < 12 {
                Meridiem::Am
            } else {
                Meridiem::Pm
            }),
        }
        if let Some(ms) = group(caps, 5) {
            let ms = parse_int(&ms[..ms.len().min(3)]);
            if ms >= 1000 {
                return None;
            }
            c.assign(MILLISECOND, ms);
        }
        if let Some(second) = group(caps, 4) {
            let second = parse_int(second);
            if second >= 60 {
                return None;
            }
            c.assign(SECOND, second);
        }

        let lower = text.to_ascii_lowercase();
        if lower.ends_with("night") {
            let hour = c.get(HOUR);
            if (6..12).contains(&hour) {
                c.assign(HOUR, hour + 12);
                c.assign_meridiem(Meridiem::Pm);
            } else if hour < 6 {
                c.assign_meridiem(Meridiem::Am);
            }
        }
        if lower.ends_with("afternoon") {
            c.assign_meridiem(Meridiem::Pm);
            let hour = c.get(HOUR);
            if (0..=6).contains(&hour) {
                c.assign(HOUR, hour + 12);
            }
        }
        if lower.ends_with("morning") {
            c.assign_meridiem(Meridiem::Am);
        }

        // checkAndReturnWithoutFollowingPattern
        if SINGLE_DIGIT_RE.is_match(text)
            || THREE_PLUS_DIGITS_RE.is_match(text)
            || DIGIT_AP_RE.is_match(text)
        {
            return None;
        }
        if let Some(ending) = ENDING_NUMBERS_RE.captures(text) {
            let ending_numbers = &ending[1];
            if ending_numbers.contains('.') && !DECIMAL_PAIRS_RE.is_match(ending_numbers) {
                return None;
            }
            let value: i64 = ending_numbers
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse()
                .unwrap_or(0);
            if value > 24 {
                return None;
            }
        }
        Some(result_at(index, text, c))
    })
}

static AGO_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        "(?i)(\\W|^)({})\\s{{0,5}}(?:ago|before|earlier)",
        time_units_pattern()
    ))
    .expect("regex")
});

fn time_unit_ago(ctx: &Context) -> Vec<ParsedResult> {
    run_parser(ctx, &AGO_RE, true, &|ctx, caps, index, text| {
        let units = reverse_time_units(&parse_time_units(&caps[2]));
        Some(result_at(
            index,
            text,
            create_relative_from_reference(&ctx.reference, &units),
        ))
    })
}

static LATER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        "(?i)(\\W|^)({})\\s{{0,5}}(?:later|after|from now|henceforth|forward|out)",
        time_units_pattern()
    ))
    .expect("regex")
});

fn time_unit_later(ctx: &Context) -> Vec<ParsedResult> {
    run_parser(ctx, &LATER_RE, true, &|ctx, caps, index, text| {
        let units = parse_time_units(&caps[2]);
        Some(result_at(
            index,
            text,
            create_relative_from_reference(&ctx.reference, &units),
        ))
    })
}

// ---------------------------------------------------------------------------
// Refiners
// ---------------------------------------------------------------------------

fn overlap_removal(results: Vec<ParsedResult>) -> Vec<ParsedResult> {
    if results.len() < 2 {
        return results;
    }
    let mut filtered = Vec::new();
    let mut iter = results.into_iter();
    let mut prev = iter.next().expect("at least two");
    for result in iter {
        if result.index < prev.end_index() {
            if result.text.len() > prev.text.len() {
                prev = result;
            }
        } else {
            filtered.push(prev);
            prev = result;
        }
    }
    filtered.push(prev);
    filtered
}

fn merge_results(
    text: &str,
    results: Vec<ParsedResult>,
    should_merge: &dyn Fn(&str, &ParsedResult, &ParsedResult) -> bool,
    merge: &dyn Fn(&str, ParsedResult, ParsedResult) -> ParsedResult,
) -> Vec<ParsedResult> {
    if results.len() < 2 {
        return results;
    }
    let mut merged = Vec::new();
    let mut iter = results.into_iter();
    let mut current = iter.next().expect("at least two");
    for next in iter {
        let between = &text[current.end_index().min(next.index)..next.index];
        if should_merge(between, &current, &next) {
            current = merge(between, current, next);
        } else {
            merged.push(current);
            current = next;
        }
    }
    merged.push(current);
    merged
}

fn merge_date_time_component(date: &Components, time: &Components) -> Components {
    let mut merged = date.clone();
    if time.is_certain(HOUR) {
        merged.assign(HOUR, time.get(HOUR));
        merged.assign(MINUTE, time.get(MINUTE));
        if time.is_certain(SECOND) {
            merged.assign(SECOND, time.get(SECOND));
            if time.is_certain(MILLISECOND) {
                merged.assign(MILLISECOND, time.get(MILLISECOND));
            } else {
                merged.imply(MILLISECOND, time.get(MILLISECOND));
            }
        } else {
            merged.imply(SECOND, time.get(SECOND));
            merged.imply(MILLISECOND, time.get(MILLISECOND));
        }
    } else {
        merged.imply(HOUR, time.get(HOUR));
        merged.imply(MINUTE, time.get(MINUTE));
        merged.imply(SECOND, time.get(SECOND));
        merged.imply(MILLISECOND, time.get(MILLISECOND));
    }
    if time.meridiem_certain {
        if let Some(m) = time.meridiem {
            merged.assign_meridiem(m);
        }
    } else if let (Some(m), None) = (time.meridiem, merged.meridiem) {
        merged.imply_meridiem(m);
    }
    if merged.meridiem == Some(Meridiem::Pm) && merged.get(HOUR) < 12 {
        let hour = merged.get(HOUR) + 12;
        if time.is_certain(HOUR) {
            merged.assign(HOUR, hour);
        } else {
            merged.imply(HOUR, hour);
        }
    }
    merged
}

fn merge_date_time_result(date_result: &ParsedResult, time_result: &ParsedResult) -> ParsedResult {
    let mut result = date_result.clone();
    result.start = merge_date_time_component(&date_result.start, &time_result.start);
    if date_result.end.is_some() || time_result.end.is_some() {
        let end_date = date_result.end.as_ref().unwrap_or(&date_result.start);
        let end_time = time_result.end.as_ref().unwrap_or(&time_result.start);
        let mut end = merge_date_time_component(end_date, end_time);
        if date_result.end.is_none() && end.rolled_date() < result.start.rolled_date() {
            let next_day = end.rolled_date() + Duration::days(1);
            if end.is_certain(DAY) {
                end.assign_similar_date(&next_day);
            } else {
                end.imply_similar_date(&next_day);
            }
        }
        result.end = Some(end);
    }
    result
}

static DATE_TIME_BETWEEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("^\\s*(T|at|after|before|on|of|,|-)?\\s*$").expect("regex"));

fn merge_date_time(text: &str, results: Vec<ParsedResult>) -> Vec<ParsedResult> {
    merge_results(
        text,
        results,
        &|between, current, next| {
            ((current.start.is_only_date() && next.start.is_only_time())
                || (next.start.is_only_date() && current.start.is_only_time()))
                && DATE_TIME_BETWEEN_RE.is_match(between)
        },
        &|between, current, next| {
            let mut result = if current.start.is_only_date() {
                merge_date_time_result(&current, &next)
            } else {
                merge_date_time_result(&next, &current)
            };
            result.index = current.index;
            result.text = format!("{}{}{}", current.text, between, next.text);
            result
        },
    )
}

static DATE_RANGE_BETWEEN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("(?i)^\\s*(to|-|–|until|through|till)\\s*$").expect("regex"));

fn merge_date_range(text: &str, results: Vec<ParsedResult>) -> Vec<ParsedResult> {
    merge_results(
        text,
        results,
        &|between, current, next| {
            current.end.is_none() && next.end.is_none() && DATE_RANGE_BETWEEN_RE.is_match(between)
        },
        &|between, mut from, mut to| {
            if !from.start.is_only_weekday_component() && !to.start.is_only_weekday_component() {
                for i in 0..7 {
                    if to.start.is_certain(i) && !from.start.is_certain(i) {
                        from.start.assign(i, to.start.get(i));
                    }
                }
                for i in 0..7 {
                    if from.start.is_certain(i) && !to.start.is_certain(i) {
                        to.start.assign(i, from.start.get(i));
                    }
                }
            }
            if from.start.rolled_date() > to.start.rolled_date() {
                let from_date = from.start.rolled_date();
                let to_date = to.start.rolled_date();
                if from.start.is_only_weekday_component() && from_date - Duration::days(7) < to_date
                {
                    let moved = from_date - Duration::days(7);
                    from.start.imply_similar_date(&moved);
                } else if to.start.is_only_weekday_component()
                    && to_date + Duration::days(7) > from_date
                {
                    let moved = to_date + Duration::days(7);
                    to.start.imply_similar_date(&moved);
                } else {
                    std::mem::swap(&mut from, &mut to);
                }
            }
            let index = from.index.min(to.index);
            let merged_text = if from.index < to.index {
                format!("{}{}{}", from.text, between, to.text)
            } else {
                format!("{}{}{}", to.text, between, from.text)
            };
            ParsedResult {
                index,
                text: merged_text,
                start: from.start,
                end: Some(to.start),
            }
        },
    )
}

static ONLY_NUMBER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("^\\d*(\\.\\d*)?$").expect("regex"));

fn unlikely_format_filter(results: Vec<ParsedResult>) -> Vec<ParsedResult> {
    results
        .into_iter()
        .filter(|r| {
            if ONLY_NUMBER_RE.is_match(&r.text.replacen(' ', "", 1)) {
                return false;
            }
            if !r.start.is_valid_date() {
                return false;
            }
            if r.end.as_ref().is_some_and(|e| !e.is_valid_date()) {
                return false;
            }
            true
        })
        .collect()
}

/// Port of `chrono.parse(text, reference)` (English casual configuration):
/// all date mentions found in `text`, ordered by position, after merging and
/// filtering.
pub(crate) fn parse(text: &str, reference: NaiveDateTime) -> Vec<ParsedResult> {
    let ctx = Context { text, reference };
    let parsers: [fn(&Context) -> Vec<ParsedResult>; 16] = [
        time_unit_casual_relative,
        relative_date_format,
        month_name,
        casual_time,
        casual_date,
        iso_format,
        slash_date,
        time_unit_within,
        month_name_little_endian,
        month_name_middle_endian,
        weekday,
        casual_year_month_day,
        slash_month,
        time_expression,
        time_unit_ago,
        time_unit_later,
    ];
    let mut results: Vec<ParsedResult> = parsers.iter().flat_map(|p| p(&ctx)).collect();
    results.sort_by_key(|r| r.index);

    let results = overlap_removal(results);
    let results = merge_date_time(text, results);
    let results = merge_date_range(text, results);
    let results = overlap_removal(results);
    unlikely_format_filter(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2021, 3, 5)
            .unwrap()
            .and_hms_milli_opt(13, 4, 5, 6)
            .unwrap()
    }

    fn first(text: &str) -> Option<(String, Option<String>)> {
        let results = parse(text, reference());
        results.first().map(|r| {
            (
                r.start.to_naive().unwrap().to_string(),
                r.end.as_ref().map(|e| e.to_naive().unwrap().to_string()),
            )
        })
    }

    #[test]
    fn parses_relative_units() {
        assert_eq!(first("7 days ago").unwrap().0, "2021-02-26 13:04:05.006");
        assert_eq!(first("1 days ago").unwrap().0, "2021-03-04 13:04:05.006");
        assert_eq!(
            first("2 weeks ago by hour").unwrap().0,
            "2021-02-19 13:04:05.006"
        );
        assert_eq!(first("23 hours ago").unwrap().0, "2021-03-04 14:04:05.006");
        assert_eq!(
            first("7 days from now").unwrap().0,
            "2021-03-12 13:04:05.006"
        );
        assert_eq!(
            first("23 hours from now").unwrap().0,
            "2021-03-06 12:04:05.006"
        );
        assert_eq!(first("1 quarter ago").unwrap().0, "2020-12-05 13:04:05.006");
        assert_eq!(first("in 3 days").unwrap().0, "2021-03-08 13:04:05.006");
        assert_eq!(first("next 5 days").unwrap().0, "2021-03-10 13:04:05.006");
        assert_eq!(first("past 2 months").unwrap().0, "2021-01-05 13:04:05.006");
        assert_eq!(first("a day ago").unwrap().0, "2021-03-04 13:04:05.006");
        assert_eq!(
            first("two weeks later").unwrap().0,
            "2021-03-19 13:04:05.006"
        );
        assert_eq!(
            first("half an hour ago").unwrap().0,
            "2021-03-05 12:34:05.006"
        );
    }

    #[test]
    fn parses_casual_references() {
        assert_eq!(first("now").unwrap().0, "2021-03-05 13:04:05.006");
        assert_eq!(first("today").unwrap().0, "2021-03-05 13:04:05.006");
        assert_eq!(first("yesterday").unwrap().0, "2021-03-04 13:04:05.006");
        assert_eq!(first("tomorrow").unwrap().0, "2021-03-06 13:04:05.006");
        assert_eq!(first("tonight").unwrap().0, "2021-03-05 22:00:00");
        assert_eq!(first("last month").unwrap().0, "2021-02-05 13:04:05.006");
        assert_eq!(first("next week").unwrap().0, "2021-03-12 13:04:05.006");
        assert_eq!(first("this month").unwrap().0, "2021-03-01 12:00:00");
    }

    #[test]
    fn parses_absolute_dates() {
        assert_eq!(first("2020-02-02").unwrap().0, "2020-02-02 12:00:00");
        assert_eq!(
            first("2020-02-02T10:30:15.250").unwrap().0,
            "2020-02-02 10:30:15.250"
        );
        assert_eq!(
            first("2020-02-02T10:30:15Z").unwrap().0,
            "2020-02-02 10:30:15"
        );
        assert_eq!(first("jan 5, 2020").unwrap().0, "2020-01-05 12:00:00");
        assert_eq!(first("5 january 2020").unwrap().0, "2020-01-05 12:00:00");
        assert_eq!(first("january 2020").unwrap().0, "2020-01-01 12:00:00");
        assert_eq!(first("1/5/2020").unwrap().0, "2020-01-05 12:00:00");
        assert_eq!(first("2020/01/05").unwrap().0, "2020-01-05 12:00:00");
        assert_eq!(first("2020-02-02 10:30").unwrap().0, "2020-02-02 10:30:00");
        assert_eq!(first("jan 5 at 5pm").unwrap().0, "2021-01-05 17:00:00");
        assert_eq!(first("monday").unwrap().0, "2021-03-08 12:00:00");
        assert_eq!(first("last friday").unwrap().0, "2021-02-26 12:00:00");
    }

    #[test]
    fn parses_ranges() {
        let (start, end) = first("2020-01-01 to 2020-01-31").unwrap();
        assert_eq!(start, "2020-01-01 12:00:00");
        assert_eq!(end.as_deref(), Some("2020-01-31 12:00:00"));
        let (start, end) = first("jan 1 - jan 5").unwrap();
        assert_eq!(start, "2021-01-01 12:00:00");
        assert_eq!(end.as_deref(), Some("2021-01-05 12:00:00"));
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("unexpected date", reference()).is_empty());
        assert!(parse("definitely not a date", reference()).is_empty());
        assert!(parse("invalid", reference()).is_empty());
        assert!(parse("2021-02-30", reference()).is_empty());
        assert!(parse("", reference()).is_empty());
    }
}
