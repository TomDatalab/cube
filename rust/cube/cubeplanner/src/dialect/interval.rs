//! `parseSqlInterval` / `splitSqlInterval`, in Rust.
//!
//! Port of `packages/cubejs-backend-shared/src/time.ts:126-162`. A SQL interval
//! is a `"<n> <unit>"` sequence — `"2 years 3 months"` — and every dialect below
//! renders it differently, so they all start here.

/// A parsed interval: `(unit, value)` in the order the units were written, as
/// the JS object preserves insertion order and several dialects iterate it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedInterval {
    parts: Vec<(String, i64)>,
}

/// SQL interval units, coarsest first (`SQL_INTERVAL_UNITS`).
const SQL_INTERVAL_UNITS: [&str; 8] = [
    "year", "quarter", "month", "week", "day", "hour", "minute", "second",
];

impl ParsedInterval {
    /// Parses `"2 years 15 months"`. An unparseable pair is skipped, matching
    /// the JS, which would store `NaN` and render nothing useful either.
    pub fn parse(interval: &str) -> Self {
        let tokens: Vec<&str> = interval.split_whitespace().collect();
        let mut parts: Vec<(String, i64)> = Vec::new();
        let mut i = 0;
        while i + 1 < tokens.len() {
            let Ok(value) = tokens[i].parse::<i64>() else {
                i += 2;
                continue;
            };
            let unit = tokens[i + 1].to_lowercase();
            let unit = unit.strip_suffix('s').unwrap_or(&unit).to_string();
            match parts.iter_mut().find(|(u, _)| *u == unit) {
                Some(slot) => slot.1 = value,
                None => parts.push((unit, value)),
            }
            i += 2;
        }
        Self { parts }
    }

    pub fn parts(&self) -> &[(String, i64)] {
        &self.parts
    }

    pub fn len(&self) -> usize {
        self.parts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    pub fn get(&self, unit: &str) -> Option<i64> {
        self.parts.iter().find(|(u, _)| u == unit).map(|(_, v)| *v)
    }

    /// `[unit]` exactly — one component, of that unit.
    pub fn only(&self, unit: &str) -> Option<i64> {
        (self.parts.len() == 1).then(|| self.get(unit)).flatten()
    }

    /// The components of `units`, in that order, if the interval has exactly
    /// those and nothing else.
    pub fn exactly(&self, units: &[&str]) -> Option<Vec<i64>> {
        if self.parts.len() != units.len() {
            return None;
        }
        units.iter().map(|unit| self.get(unit)).collect()
    }
}

/// One single-unit interval string per component, coarsest first
/// (`splitSqlInterval`). Units outside `SQL_INTERVAL_UNITS` keep their place at
/// the end, so a caller that cannot render one still sees it.
pub fn split_sql_interval(interval: &str) -> Vec<String> {
    let parsed = ParsedInterval::parse(interval);
    let mut ordered: Vec<&(String, i64)> = SQL_INTERVAL_UNITS
        .iter()
        .filter_map(|unit| parsed.parts.iter().find(|(u, _)| u == unit))
        .collect();
    ordered.extend(
        parsed
            .parts
            .iter()
            .filter(|(u, _)| !SQL_INTERVAL_UNITS.contains(&u.as_str())),
    );
    ordered
        .into_iter()
        .map(|(unit, value)| format!("{value} {unit}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plural_and_negative_units() {
        let parsed = ParsedInterval::parse("-2 months 5 days -10 hours");
        assert_eq!(
            parsed.parts(),
            &[
                ("month".to_string(), -2),
                ("day".to_string(), 5),
                ("hour".to_string(), -10)
            ]
        );
        assert_eq!(parsed.only("month"), None);
        assert_eq!(ParsedInterval::parse("6 months").only("month"), Some(6));
        assert_eq!(
            ParsedInterval::parse("1 year 2 months").exactly(&["year", "month"]),
            Some(vec![1, 2])
        );
    }

    #[test]
    fn splits_coarsest_first() {
        assert_eq!(
            split_sql_interval("5 days 2 years 3 hours"),
            vec!["2 year", "5 day", "3 hour"]
        );
    }
}
