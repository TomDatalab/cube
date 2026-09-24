//! Local evaluation of `every` based refresh keys.
//!
//! Port of `evaluateLocalRefreshKey` / `isValidLocalRefreshKey` (`QO/utils.ts:35-59`),
//! reached through `CUBEJS_REFRESH_KEY_LOCAL_TIME`.

use cubecache::KeyValue;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Everything needed to evaluate an `every` based refreshKey without touching a database:
/// `FLOOR((utcOffset + unixTimestamp - dayOffset) / interval)`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalRefreshKeyDescriptor {
    pub interval: f64,
    pub utc_offset: f64,
    pub day_offset: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cron: Option<bool>,
}

impl LocalRefreshKeyDescriptor {
    pub fn new(interval: f64, utc_offset: f64, day_offset: f64) -> Self {
        Self {
            interval,
            utc_offset,
            day_offset,
            cron: None,
        }
    }

    /// The `JSON.stringify` shape of the descriptor, for the refresh key identity hash.
    ///
    /// Only integers occur here — the compiler emits whole seconds — so the numbers are
    /// emitted as integers, the way `JSON.stringify` writes an integral JavaScript number.
    pub fn to_key_value(&self) -> KeyValue {
        let mut fields = vec![
            ("interval".into(), KeyValue::Int(self.interval as i64)),
            ("utcOffset".into(), KeyValue::Int(self.utc_offset as i64)),
            ("dayOffset".into(), KeyValue::Int(self.day_offset as i64)),
        ];

        if let Some(cron) = self.cron {
            fields.push(("cron".into(), KeyValue::Bool(cron)));
        }

        KeyValue::Object(fields)
    }
}

/// `isValidLocalRefreshKey` (`QO/utils.ts:55-59`): anything that would produce a garbage
/// key keeps the refresh key on the SQL path.
pub fn is_valid_local_refresh_key(descriptor: Option<&LocalRefreshKeyDescriptor>) -> bool {
    match descriptor {
        Some(descriptor) => {
            descriptor.interval.is_finite()
                && descriptor.interval > 0.0
                && descriptor.utc_offset.is_finite()
                && descriptor.day_offset.is_finite()
        }
        None => false,
    }
}

/// `evaluateLocalRefreshKey` (`QO/utils.ts:45-53`).
///
/// The value is a string because `contentVersion` and the query cache `renewalKey` hash it
/// through `JSON.stringify`, where `2980310` and `"2980310"` are different keys — and
/// because the SQL path yields a string too, Cube Store carrying every column as one.
pub fn evaluate_local_refresh_key(
    descriptor: &LocalRefreshKeyDescriptor,
    now_ms: i64,
) -> Vec<Value> {
    let value = ((descriptor.utc_offset + now_ms as f64 / 1000.0 - descriptor.day_offset)
        / descriptor.interval)
        .floor();

    vec![json!({ "refresh_key": format!("{}", value as i64) })]
}

/// [`evaluate_local_refresh_key`] against the current clock.
pub fn evaluate_local_refresh_key_now(descriptor: &LocalRefreshKeyDescriptor) -> Vec<Value> {
    evaluate_local_refresh_key(descriptor, chrono::Utc::now().timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEN_MINUTES: LocalRefreshKeyDescriptor = LocalRefreshKeyDescriptor {
        interval: 600.0,
        utc_offset: 0.0,
        day_offset: 0.0,
        cron: None,
    };

    fn key(descriptor: &LocalRefreshKeyDescriptor, now_ms: i64) -> String {
        evaluate_local_refresh_key(descriptor, now_ms)[0]["refresh_key"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Ported from `test/unit/utils.test.ts`.
    #[test]
    fn returns_the_same_row_shape_as_the_sql_query() {
        assert_eq!(
            evaluate_local_refresh_key(&TEN_MINUTES, 600_000),
            vec![json!({ "refresh_key": "1" })]
        );
        assert_eq!(key(&TEN_MINUTES, 6_000_000), "10");
    }

    #[test]
    fn changes_exactly_at_the_interval_boundary() {
        assert_eq!(key(&TEN_MINUTES, 599_999), "0");
        assert_eq!(key(&TEN_MINUTES, 600_000), "1");
        assert_eq!(key(&TEN_MINUTES, 1_199_999), "1");
        assert_eq!(key(&TEN_MINUTES, 1_200_000), "2");
    }

    #[test]
    fn sub_second_precision_never_changes_the_result() {
        for ms in [0, 1, 250, 500, 999] {
            assert_eq!(key(&TEN_MINUTES, 600_000 + ms), key(&TEN_MINUTES, 600_000));
        }
    }

    #[test]
    fn applies_a_negative_utc_offset() {
        let hourly = LocalRefreshKeyDescriptor::new(3600.0, -28800.0, 0.0);

        assert_eq!(key(&hourly, 28_800_000), "0");
    }

    #[test]
    fn applies_day_offset_for_cron_based_keys() {
        let daily = LocalRefreshKeyDescriptor {
            interval: 86400.0,
            utc_offset: 0.0,
            day_offset: 36000.0,
            cron: Some(true),
        };

        assert_eq!(key(&daily, 35_999_000), "-1");
        assert_eq!(key(&daily, 36_000_000), "0");
        assert_eq!(key(&daily, 36_000_000 + 86_400_000), "1");
    }

    #[test]
    fn defaults_to_the_current_clock() {
        let before = (chrono::Utc::now().timestamp_millis() as f64 / 1000.0 / 600.0).floor() as i64;
        let value: i64 = evaluate_local_refresh_key_now(&TEN_MINUTES)[0]["refresh_key"]
            .as_str()
            .unwrap()
            .parse()
            .unwrap();
        let after = (chrono::Utc::now().timestamp_millis() as f64 / 1000.0 / 600.0).floor() as i64;

        assert!(value >= before && value <= after);
    }

    #[test]
    fn rejects_anything_that_would_produce_a_garbage_key() {
        assert!(is_valid_local_refresh_key(Some(&TEN_MINUTES)));
        assert!(is_valid_local_refresh_key(Some(
            &LocalRefreshKeyDescriptor::new(1.0, -28800.0, 36000.0)
        )));

        assert!(!is_valid_local_refresh_key(None));
        assert!(!is_valid_local_refresh_key(Some(
            &LocalRefreshKeyDescriptor::new(0.0, 0.0, 0.0)
        )));
        assert!(!is_valid_local_refresh_key(Some(
            &LocalRefreshKeyDescriptor::new(-600.0, 0.0, 0.0)
        )));
        assert!(!is_valid_local_refresh_key(Some(
            &LocalRefreshKeyDescriptor::new(f64::NAN, 0.0, 0.0)
        )));
        assert!(!is_valid_local_refresh_key(Some(
            &LocalRefreshKeyDescriptor::new(f64::INFINITY, 0.0, 0.0)
        )));
        assert!(!is_valid_local_refresh_key(Some(
            &LocalRefreshKeyDescriptor::new(600.0, f64::NAN, 0.0)
        )));
        assert!(!is_valid_local_refresh_key(Some(
            &LocalRefreshKeyDescriptor::new(600.0, 0.0, f64::NAN)
        )));
    }
}
