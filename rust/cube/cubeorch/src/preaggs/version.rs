//! Pre-aggregation version hashing, table naming and version entry parsing.
//!
//! Port of `version`, `getStructureVersion`, `tablesToVersionEntries`
//! (`QO/PreAggregations.ts:28-87, 218-246`), `PreAggregations.targetTableName`
//! (`:810-816`), `PreAggregationLoader.contentVersion` (`QO/PreAggregationLoader.ts:376-389`)
//! and `PreAggregationPartitionRangeLoader.partitionTableName` (`:584-604`).
//!
//! Everything here is pure, so it is golden tested against the JavaScript implementation.

use cubecache::KeyValue;
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};

use crate::types::PreAggregationDescription;

/// The alphabet `version` encodes the first five md5 bytes with.
const HASH_CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz012345";

/// `version(cacheKey)` (`QO/PreAggregations.ts:34-60`).
///
/// A base32 rendering of the first five bytes of `md5(JSON.stringify(cacheKey))` over a
/// letters-first alphabet, so that the result is always a valid identifier prefix. The bit
/// shuffling is reproduced verbatim: it emits characters while the residue still holds more
/// than five bits, which makes the output length input dependent (7 or 8 characters).
pub fn version(cache_key: &KeyValue) -> String {
    let digest = Md5::digest(cache_key.to_json().as_bytes());

    let mut result = String::new();
    let mut residue: u32 = 0;
    let mut shift_counter: i32 = 0;

    for byte in digest.iter().take(5) {
        shift_counter += 8;
        // `(x | y) >>> 0` in the source: JavaScript's bitwise operators work on signed
        // 32 bit integers and the unsigned shift puts the value back into u32 range, which
        // is what Rust's u32 does natively.
        residue |= u32::from(*byte) << ((shift_counter - 8) as u32 & 31);

        while residue >> 5 != 0 {
            result.push(HASH_CHARSET[(residue % 32) as usize] as char);
            shift_counter -= 5;
            residue >>= 5;
        }
    }

    result.push(HASH_CHARSET[(residue % 32) as usize] as char);

    result
}

/// The `[structureVersionLoadSql || loadSql, indexesSql?, streamOffset?, outputColumnTypes?]`
/// prefix both version hashes are built from.
fn version_array(pre_aggregation: &PreAggregationDescription) -> Vec<KeyValue> {
    let load_sql = pre_aggregation
        .structure_version_load_sql
        .as_ref()
        .or(pre_aggregation.load_sql.as_ref())
        .map(|query| query.to_key_value())
        .unwrap_or(KeyValue::Null);

    let mut items = vec![load_sql];

    // `if (preAggregation.indexesSql?.length)` — an empty list does not take part.
    if let Some(indexes_sql) = &pre_aggregation.indexes_sql {
        let is_non_empty = match indexes_sql {
            serde_json::Value::Array(items) => !items.is_empty(),
            serde_json::Value::Null => false,
            _ => true,
        };

        if is_non_empty {
            items.push(KeyValue::from(indexes_sql));
        }
    }

    if let Some(stream_offset) = &pre_aggregation.stream_offset {
        if is_truthy(stream_offset) {
            items.push(KeyValue::from(stream_offset));
        }
    }

    if let Some(output_column_types) = &pre_aggregation.output_column_types {
        if is_truthy(output_column_types) {
            items.push(KeyValue::from(output_column_types));
        }
    }

    items
}

fn is_truthy(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::Number(value) => value.as_f64().is_some_and(|value| value != 0.0),
        serde_json::Value::String(value) => !value.is_empty(),
        _ => true,
    }
}

/// `getStructureVersion(preAggregation)` (`QO/PreAggregations.ts:74-87`).
///
/// A single element array collapses to its element, which is why a pre-aggregation without
/// indexes hashes over the bare `loadSql` tuple rather than over `[loadSql]`.
pub fn get_structure_version(pre_aggregation: &PreAggregationDescription) -> String {
    let mut items = version_array(pre_aggregation);

    if items.len() == 1 {
        version(&items.remove(0))
    } else {
        version(&KeyValue::Array(items))
    }
}

/// `PreAggregationLoader.contentVersion(invalidationKeys)`
/// (`QO/PreAggregationLoader.ts:376-389`) — the structure array with the invalidation key
/// values appended, and never collapsed to a single element.
pub fn content_version(
    pre_aggregation: &PreAggregationDescription,
    invalidation_keys: &KeyValue,
) -> String {
    let mut items = version_array(pre_aggregation);
    items.push(invalidation_keys.clone());

    version(&KeyValue::Array(items))
}

/// `Math.floor(time / 1000).toString(32)`.
pub fn encode_time_stamp(time_ms: i64) -> String {
    to_radix_32((time_ms as f64 / 1000.0).floor() as i64)
}

/// `parseInt(time, 32) * 1000`. An unparseable group yields `None`, where JavaScript would
/// carry a `NaN` through the version entry.
pub fn decode_time_stamp(encoded: &str) -> Option<i64> {
    from_radix_32(encoded).map(|seconds| seconds * 1000)
}

fn to_radix_32(mut value: i64) -> String {
    if value == 0 {
        return "0".to_string();
    }

    let negative = value < 0;
    if negative {
        value = -value;
    }

    let mut digits = Vec::new();
    while value > 0 {
        let digit = (value % 32) as u8;
        digits.push(if digit < 10 {
            b'0' + digit
        } else {
            b'a' + (digit - 10)
        });
        value /= 32;
    }

    if negative {
        digits.push(b'-');
    }
    digits.reverse();

    String::from_utf8(digits).expect("radix 32 digits are ascii")
}

/// `parseInt(value, 32)`: leading digits are consumed and the rest is ignored, an empty
/// prefix is `NaN`.
fn from_radix_32(value: &str) -> Option<i64> {
    let (negative, body) = match value.strip_prefix('-') {
        Some(body) => (true, body),
        None => (false, value),
    };

    let mut result: i64 = 0;
    let mut consumed = 0;

    for character in body.chars() {
        let digit = match character.to_ascii_lowercase() {
            c @ '0'..='9' => c as i64 - '0' as i64,
            c @ 'a'..='v' => c as i64 - 'a' as i64 + 10,
            _ => break,
        };

        result = result.checked_mul(32)?.checked_add(digit)?;
        consumed += 1;
    }

    if consumed == 0 {
        return None;
    }

    Some(if negative { -result } else { result })
}

/// The `last_updated_at` of a version entry, which is a timestamp everywhere except in the
/// "no partitions built" message, where it is the literal `*`
/// (`PreAggregations.noPreAggregationPartitionsBuiltMessage`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableTimestamp {
    At(i64),
    Wildcard,
}

/// `PreAggregations.targetTableName(versionEntry)` (`QO/PreAggregations.ts:810-816`).
pub fn target_table_name(
    table_name: &str,
    content_version: &str,
    structure_version: &str,
    last_updated_at: TableTimestamp,
    naming_version: Option<u32>,
) -> String {
    let suffix = match (naming_version, last_updated_at) {
        (_, TableTimestamp::Wildcard) => "*".to_string(),
        (Some(2), TableTimestamp::At(time)) => encode_time_stamp(time),
        (_, TableTimestamp::At(time)) => time.to_string(),
    };

    format!("{table_name}_{content_version}_{structure_version}_{suffix}")
}

/// A pre-aggregation table as `tablesToVersionEntries` decodes it
/// (`VersionEntry`, `QO/PreAggregations.ts:89-96`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionEntry {
    pub table_name: String,
    pub content_version: String,
    pub structure_version: String,
    pub last_updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_range_end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub naming_version: Option<u32>,
}

impl VersionEntry {
    /// [`target_table_name`] of this entry.
    pub fn target_table_name(&self) -> String {
        target_table_name(
            &self.table_name,
            &self.content_version,
            &self.structure_version,
            TableTimestamp::At(self.last_updated_at),
            self.naming_version,
        )
    }

    /// `${table_name}_${structure_version}`, the `byStructure` index key.
    pub fn structure_key(&self) -> String {
        format!("{}_{}", self.table_name, self.structure_version)
    }

    /// `${table_name}_${content_version}`, the `byContent` index key.
    pub fn content_key(&self) -> String {
        format!("{}_{}", self.table_name, self.content_version)
    }
}

/// One row of the pre-aggregation schema listing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableCacheEntry {
    pub table_name: String,
    pub build_range_end: Option<String>,
}

impl TableCacheEntry {
    pub fn new(table_name: impl Into<String>) -> Self {
        Self {
            table_name: table_name.into(),
            build_range_end: None,
        }
    }
}

/// Splits a table name on its last three underscores, which is what the greedy
/// `/(.+)_(.+)_(.+)_(.+)/` of `tablesToVersionEntries` does: the first group takes as much
/// as it can while leaving three non-empty groups behind.
fn split_version_suffixes(table_name: &str) -> Option<(&str, &str, &str, &str)> {
    let mut parts = table_name.rsplitn(4, '_');
    let last_updated_at = parts.next()?;
    let structure_version = parts.next()?;
    let content_version = parts.next()?;
    let name = parts.next()?;

    if name.is_empty()
        || content_version.is_empty()
        || structure_version.is_empty()
        || last_updated_at.is_empty()
    {
        return None;
    }

    Some((name, content_version, structure_version, last_updated_at))
}

/// `tablesToVersionEntries(schema, tables)` (`QO/PreAggregations.ts:218-246`).
///
/// Tables that do not carry the four name groups are dropped, and the result is sorted by
/// `last_updated_at` descending, stably, so that two entries built within the same second
/// keep the order the driver listed them in.
pub fn tables_to_version_entries(schema: &str, tables: &[TableCacheEntry]) -> Vec<VersionEntry> {
    let mut entries: Vec<VersionEntry> = tables
        .iter()
        .filter_map(|table| {
            let (name, content_version, structure_version, last_updated_at) =
                split_version_suffixes(&table.table_name)?;

            // A group shorter than 13 characters cannot be a millisecond timestamp, so it is
            // the base32 encoding of naming version 2.
            let (last_updated_at, naming_version) = if last_updated_at.chars().count() < 13 {
                (decode_time_stamp(last_updated_at).unwrap_or(0), Some(2))
            } else {
                (last_updated_at.parse::<i64>().unwrap_or(0), None)
            };

            Some(VersionEntry {
                table_name: format!("{schema}.{name}"),
                content_version: content_version.to_string(),
                structure_version: structure_version.to_string(),
                last_updated_at,
                build_range_end: table.build_range_end.clone(),
                naming_version,
            })
        })
        .collect();

    entries.sort_by_key(|entry| std::cmp::Reverse(entry.last_updated_at));

    entries
}

/// `getLastUpdatedAtTimestamp` (`QO/PreAggregations.ts:63-72`) — the **oldest** timestamp,
/// because a result is only as fresh as the stalest pre-aggregation behind it.
pub fn get_last_updated_at_timestamp(timestamps: &[Option<i64>]) -> Option<i64> {
    timestamps.iter().flatten().copied().min()
}

/// `PreAggregationPartitionRangeLoader.partitionTableName` (`:584-604`).
pub fn partition_table_name(
    table_name: &str,
    partition_granularity: &str,
    date_range_start: &str,
) -> String {
    let date_len_cut = match partition_granularity {
        "hour" => 13,
        "minute" => 16,
        _ => 10,
    };

    let suffix: String = date_range_start
        .chars()
        .take(date_len_cut)
        .filter(|character| !matches!(character, '-' | 'T' | ':'))
        .collect();

    format!("{table_name}{suffix}")
}

/// `PreAggregations.noPreAggregationPartitionsBuiltMessage` (`:818-830`).
pub fn no_pre_aggregation_partitions_built_message(
    pre_aggregations: &[PreAggregationDescription],
) -> String {
    let expected: Vec<String> = pre_aggregations
        .iter()
        .map(|pre_aggregation| {
            target_table_name(
                &pre_aggregation.table_name,
                "*",
                &get_structure_version(pre_aggregation),
                TableTimestamp::Wildcard,
                Some(2),
            )
        })
        .collect();

    format!(
        "No pre-aggregation partitions were built yet for the pre-aggregation serving this query \
         and this API instance wasn't set up to build pre-aggregations. Please make sure your \
         refresh worker is configured correctly, running, pre-aggregation tables are built and all \
         pre-aggregation refresh settings like timezone match. Expected table name patterns: {}",
        expected.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::types::QueryWithParams;

    fn pre_aggregation(load_sql: QueryWithParams) -> PreAggregationDescription {
        PreAggregationDescription {
            table_name: "stb_pre_aggregations.orders".to_string(),
            load_sql: Some(load_sql),
            ..Default::default()
        }
    }

    /// Every expectation below was produced by running the repository's own implementation
    /// under Node as an oracle, from the repository root:
    ///
    /// ```text
    /// node -e "
    ///   const P = require('./packages/cubejs-query-orchestrator/dist/src/orchestrator/PreAggregations');
    ///   console.log(P.version(['SELECT 1', []]));
    /// "
    /// ```
    #[test]
    fn golden_version_hashes() {
        assert_eq!(version(&KeyValue::Str("abc".into())), "lpwj5ddw");
        assert_eq!(version(&KeyValue::sql("SELECT 1", &[])), "oa452mzj");
        assert_eq!(
            version(&KeyValue::sql(
                "SELECT * FROM t WHERE a = $1",
                &["x".to_string()]
            )),
            "peotniyl"
        );
        assert_eq!(version(&KeyValue::Array(vec![])), "xoucx2ar");
        assert_eq!(
            version(&KeyValue::Array(vec![KeyValue::Str("a".into())])),
            "msfxlal2"
        );
        assert_eq!(
            version(&KeyValue::Array(vec![
                KeyValue::sql("SELECT 1", &[]),
                KeyValue::Array(vec![KeyValue::Array(vec![
                    KeyValue::Str("idx1".into()),
                    KeyValue::Str("CREATE INDEX ...".into()),
                ])]),
            ])),
            "d0hujx0j"
        );
        assert_eq!(
            version(&KeyValue::Array(vec![KeyValue::Str("ünïcødé ☃".into())])),
            "jyf3a15k"
        );
        assert_eq!(
            version(&KeyValue::Array(vec![
                KeyValue::sql("SELECT 1", &[]),
                KeyValue::strings(&["k1".to_string(), "k2".to_string()]),
            ])),
            "5q4ybne2"
        );
    }

    #[test]
    fn golden_structure_versions() {
        let load_sql = QueryWithParams::new("CREATE TABLE x AS SELECT 1", vec![]);

        assert_eq!(
            get_structure_version(&pre_aggregation(load_sql.clone())),
            "i0insxys"
        );

        let mut with_indexes = pre_aggregation(load_sql.clone());
        with_indexes.indexes_sql =
            Some(json!([{ "indexName": "i1", "sql": ["CREATE INDEX i1", []] }]));
        assert_eq!(get_structure_version(&with_indexes), "qsxde4hy");

        let mut override_sql = pre_aggregation(QueryWithParams::new("other", vec![]));
        override_sql.structure_version_load_sql =
            Some(QueryWithParams::new("SVLS", vec!["p".to_string()]));
        assert_eq!(get_structure_version(&override_sql), "elqod4f4");

        let mut streamed = pre_aggregation(QueryWithParams::new("L", vec![]));
        streamed.stream_offset = Some(json!("earliest"));
        assert_eq!(get_structure_version(&streamed), "cvmkf3wm");

        let mut typed = pre_aggregation(QueryWithParams::new("L", vec![]));
        typed.output_column_types = Some(json!([{ "member": "a", "type": "string" }]));
        assert_eq!(get_structure_version(&typed), "gop5r54i");

        // An empty `indexesSql` is falsy in the source and does not take part.
        let mut empty_indexes = pre_aggregation(QueryWithParams::new("L", vec![]));
        empty_indexes.indexes_sql = Some(json!([]));
        assert_eq!(get_structure_version(&empty_indexes), "fvw0104l");
    }

    #[test]
    fn golden_content_versions() {
        let invalidation_keys = KeyValue::Array(vec![KeyValue::Array(vec![
            KeyValue::Array(vec![
                KeyValue::Str("SELECT MAX(id) FROM t".into()),
                KeyValue::Array(vec![]),
                KeyValue::Object(vec![("external".into(), KeyValue::Bool(false))]),
            ]),
            KeyValue::Array(vec![KeyValue::Object(vec![(
                "max".into(),
                KeyValue::Str("5".into()),
            )])]),
            KeyValue::Str("hash".into()),
        ])]);

        assert_eq!(
            content_version(
                &pre_aggregation(QueryWithParams::new("CREATE TABLE x AS SELECT 1", vec![])),
                &invalidation_keys
            ),
            "pper1x0b"
        );

        let mut indexed = pre_aggregation(QueryWithParams::new("L", vec![]));
        indexed.indexes_sql = Some(json!([{ "indexName": "i1", "sql": ["CREATE INDEX i1", []] }]));
        assert_eq!(
            content_version(
                &indexed,
                &KeyValue::Array(vec![KeyValue::Array(vec![
                    KeyValue::Int(1),
                    KeyValue::Int(2)
                ])])
            ),
            "4yz3rc3z"
        );
    }

    #[test]
    fn golden_target_table_names() {
        assert_eq!(
            target_table_name(
                "orders_number_and_count20191101",
                "kjypcoio",
                "5yftl5il",
                TableTimestamp::At(1600329890789),
                None
            ),
            "orders_number_and_count20191101_kjypcoio_5yftl5il_1600329890789"
        );
        assert_eq!(
            target_table_name(
                "orders_number_and_count20191101",
                "kjypcoio",
                "5yftl5il",
                TableTimestamp::At(1600329890789),
                Some(2)
            ),
            "orders_number_and_count20191101_kjypcoio_5yftl5il_1fm6652"
        );
        assert_eq!(
            target_table_name("t", "c", "s", TableTimestamp::Wildcard, Some(2)),
            "t_c_s_*"
        );
        assert_eq!(
            target_table_name("t", "c", "s", TableTimestamp::At(0), Some(2)),
            "t_c_s_0"
        );
    }

    #[test]
    fn time_stamp_round_trip() {
        assert_eq!(encode_time_stamp(1600329890789), "1fm6652");
        assert_eq!(decode_time_stamp("1fm6652"), Some(1600329890000));
        assert_eq!(encode_time_stamp(0), "0");
        assert_eq!(decode_time_stamp("0"), Some(0));
        // `parseInt` stops at the first digit outside the radix and reports nothing when the
        // very first character already is.
        assert_eq!(decode_time_stamp("1fz"), Some(47000));
        assert_eq!(decode_time_stamp("zzz"), None);
        assert_eq!(decode_time_stamp(""), None);
    }

    #[test]
    fn golden_tables_to_version_entries() {
        let entries = tables_to_version_entries(
            "stb_pre_aggregations",
            &[
                TableCacheEntry::new(
                    "orders_number_and_count20191101_kjypcoio_5yftl5il_1593709044209",
                ),
                TableCacheEntry::new("orders_number_and_count20191101_kjypcoio_5yftl5il_1fm6652"),
                TableCacheEntry {
                    table_name: "orders_d_aaa_bbb_1fm6652".to_string(),
                    build_range_end: Some("2020-01-01T00:00:00.000".to_string()),
                },
                TableCacheEntry::new("no_match"),
            ],
        );

        assert_eq!(
            entries,
            vec![
                VersionEntry {
                    table_name: "stb_pre_aggregations.orders_number_and_count20191101".to_string(),
                    content_version: "kjypcoio".to_string(),
                    structure_version: "5yftl5il".to_string(),
                    last_updated_at: 1600329890000,
                    build_range_end: None,
                    naming_version: Some(2),
                },
                VersionEntry {
                    table_name: "stb_pre_aggregations.orders_d".to_string(),
                    content_version: "aaa".to_string(),
                    structure_version: "bbb".to_string(),
                    last_updated_at: 1600329890000,
                    build_range_end: Some("2020-01-01T00:00:00.000".to_string()),
                    naming_version: Some(2),
                },
                VersionEntry {
                    table_name: "stb_pre_aggregations.orders_number_and_count20191101".to_string(),
                    content_version: "kjypcoio".to_string(),
                    structure_version: "5yftl5il".to_string(),
                    last_updated_at: 1593709044209,
                    build_range_end: None,
                    naming_version: None,
                },
            ]
        );

        // The naming version 2 entry and the millisecond one round trip back to their names.
        assert_eq!(
            entries[0].target_table_name(),
            "stb_pre_aggregations.orders_number_and_count20191101_kjypcoio_5yftl5il_1fm6652"
        );
        assert_eq!(
            entries[2].target_table_name(),
            "stb_pre_aggregations.orders_number_and_count20191101_kjypcoio_5yftl5il_1593709044209"
        );
    }

    #[test]
    fn version_entry_name_groups_are_taken_from_the_right() {
        assert_eq!(
            split_version_suffixes("a_b_c_d_e"),
            Some(("a_b", "c", "d", "e"))
        );
        assert_eq!(
            split_version_suffixes("a__b_c_d"),
            Some(("a_", "b", "c", "d"))
        );
        assert_eq!(split_version_suffixes("no_match"), None);
        assert_eq!(split_version_suffixes("a_b_c_"), None);
        assert_eq!(split_version_suffixes("_a_b_c"), None);
    }

    #[test]
    fn last_updated_at_timestamp_is_the_oldest() {
        assert_eq!(
            get_last_updated_at_timestamp(&[Some(30), None, Some(10), Some(20)]),
            Some(10)
        );
        assert_eq!(get_last_updated_at_timestamp(&[None, None]), None);
        assert_eq!(get_last_updated_at_timestamp(&[]), None);
    }

    /// Ported from `test/unit/PreAggregations.test.ts`, `describe('partitionTableName')`.
    #[test]
    fn partition_table_names_per_granularity() {
        let start = "2024-01-05T12:34:56.789";

        assert_eq!(
            partition_table_name("test_table", "day", start),
            "test_table20240105"
        );
        assert_eq!(
            partition_table_name("test_table", "hour", start),
            "test_table2024010512"
        );
        assert_eq!(
            partition_table_name("test_table", "minute", start),
            "test_table202401051234"
        );
    }

    #[test]
    fn golden_no_partitions_built_message() {
        let message = no_pre_aggregation_partitions_built_message(&[PreAggregationDescription {
            table_name: "s.t".to_string(),
            load_sql: Some(QueryWithParams::new("L", vec![])),
            ..Default::default()
        }]);

        assert!(message.ends_with("Expected table name patterns: s.t_*_fvw0104l_*"));
        assert!(message.starts_with("No pre-aggregation partitions were built yet"));
    }
}
