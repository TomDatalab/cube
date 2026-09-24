//! Cache and queue key construction and hashing.
//!
//! Port of `packages/cubejs-query-orchestrator/src/orchestrator/utils.ts` (`getCacheHash`),
//! `QueryCache.queryCacheKey` and `QueryCache.refreshKeyIdentity`.
//!
//! The hash is `md5(JSON.stringify(key))`, so the JSON serialization has to match
//! JavaScript's byte for byte. That is why keys are modelled as an ordered
//! [`KeyValue`] tree instead of `serde_json::Value`: `serde_json::Map` is a `BTreeMap`
//! unless the `preserve_order` feature is on, and JSON objects inside a key would then
//! be emitted in sorted order while `JSON.stringify` emits them in insertion order.
//! `[{"b":2,"a":1}]` and `[{"a":1,"b":2}]` hash differently, so the two are not
//! interchangeable.

use std::sync::OnceLock;

use md5::{Digest, Md5};

/// Cache key namespace of the SQL query result cache (`QueryCache.queryCacheKey`).
pub const SQL_QUERY_RESULT: &str = "SQL_QUERY_RESULT";

/// A JSON value which keeps object fields in insertion order.
///
/// Only the shapes which actually occur in Cube's cache and queue keys are modelled.
/// Floating point numbers are deliberately left out: `JSON.stringify` formats them with
/// JavaScript's `Number::toString` (`1e+21`, not `1e21`), which `serde_json` does not
/// reproduce. Every number inside a key Cube builds is an integer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeyValue {
    /// `null` — also what `JSON.stringify` writes for `undefined` inside an array.
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Array(Vec<KeyValue>),
    Object(Vec<(String, KeyValue)>),
}

impl KeyValue {
    /// `["<sql>", ["<param>", …]]` — the `QueryWithParams` tuple of the Node code.
    pub fn sql(sql: impl Into<String>, params: &[String]) -> Self {
        KeyValue::Array(vec![
            KeyValue::Str(sql.into()),
            KeyValue::Array(params.iter().cloned().map(KeyValue::Str).collect()),
        ])
    }

    /// `["<string>", …]`
    pub fn strings(values: &[String]) -> Self {
        KeyValue::Array(values.iter().cloned().map(KeyValue::Str).collect())
    }

    /// Serializes exactly like `JSON.stringify` does for these shapes.
    pub fn to_json(&self) -> String {
        let mut out = String::new();
        self.write_json(&mut out);
        out
    }

    fn write_json(&self, out: &mut String) {
        match self {
            KeyValue::Null => out.push_str("null"),
            KeyValue::Bool(true) => out.push_str("true"),
            KeyValue::Bool(false) => out.push_str("false"),
            KeyValue::Int(v) => out.push_str(&v.to_string()),
            // `serde_json` escapes `"`, `\` and the control characters the same way
            // `JSON.stringify` does and leaves everything else (including non-ASCII) as is.
            KeyValue::Str(v) => out.push_str(&serde_json::Value::String(v.clone()).to_string()),
            KeyValue::Array(items) => {
                out.push('[');
                for (idx, item) in items.iter().enumerate() {
                    if idx > 0 {
                        out.push(',');
                    }
                    item.write_json(out);
                }
                out.push(']');
            }
            KeyValue::Object(fields) => {
                out.push('{');
                for (idx, (name, value)) in fields.iter().enumerate() {
                    if idx > 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::Value::String(name.clone()).to_string());
                    out.push(':');
                    value.write_json(out);
                }
                out.push('}');
            }
        }
    }
}

impl From<&KeyValue> for serde_json::Value {
    fn from(value: &KeyValue) -> Self {
        match value {
            KeyValue::Null => serde_json::Value::Null,
            KeyValue::Bool(v) => serde_json::Value::Bool(*v),
            KeyValue::Int(v) => serde_json::Value::Number((*v).into()),
            KeyValue::Str(v) => serde_json::Value::String(v.clone()),
            KeyValue::Array(items) => {
                serde_json::Value::Array(items.iter().map(serde_json::Value::from).collect())
            }
            KeyValue::Object(fields) => serde_json::Value::Object(
                fields
                    .iter()
                    .map(|(name, value)| (name.clone(), serde_json::Value::from(value)))
                    .collect(),
            ),
        }
    }
}

impl From<&serde_json::Value> for KeyValue {
    /// Best effort inverse of [`From<&KeyValue> for serde_json::Value`].
    ///
    /// Object field order is whatever `serde_json::Map` yields (sorted, unless the crate
    /// is built with `preserve_order`), so a key which contains an object and is round
    /// tripped through `serde_json::Value` may hash differently than the original.
    fn from(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => KeyValue::Null,
            serde_json::Value::Bool(v) => KeyValue::Bool(*v),
            serde_json::Value::Number(v) => KeyValue::Int(v.as_i64().unwrap_or_default()),
            serde_json::Value::String(v) => KeyValue::Str(v.clone()),
            serde_json::Value::Array(items) => {
                KeyValue::Array(items.iter().map(KeyValue::from).collect())
            }
            serde_json::Value::Object(fields) => KeyValue::Object(
                fields
                    .iter()
                    .map(|(name, value)| (name.clone(), KeyValue::from(value)))
                    .collect(),
            ),
        }
    }
}

/// A cache or queue key.
///
/// Mirrors the Node `QueryKey`/`CacheKey` union: either a bare string or an array,
/// with an out of band `persistent` flag. The flag is a property on the JavaScript
/// array, so it never takes part in the serialization — it only decides whether the
/// hash is suffixed with the process uid.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CacheKey {
    Str(String),
    List {
        items: Vec<KeyValue>,
        persistent: bool,
    },
}

impl CacheKey {
    pub fn string(value: impl Into<String>) -> Self {
        CacheKey::Str(value.into())
    }

    pub fn list(items: Vec<KeyValue>) -> Self {
        CacheKey::List {
            items,
            persistent: false,
        }
    }

    /// `["<sql>", ["<param>", …]]` — the shape of a queue key for a SQL query.
    pub fn sql(sql: impl Into<String>, params: &[String]) -> Self {
        CacheKey::list(vec![
            KeyValue::Str(sql.into()),
            KeyValue::Array(params.iter().cloned().map(KeyValue::Str).collect()),
        ])
    }

    /// Sets the `persistent` flag. A string key can never be persistent, matching
    /// `typeof queryKey === 'object'` in `getCacheHash`.
    #[must_use]
    pub fn with_persistent(self, persistent: bool) -> Self {
        match self {
            CacheKey::Str(value) => CacheKey::Str(value),
            CacheKey::List { items, .. } => CacheKey::List { items, persistent },
        }
    }

    pub fn is_persistent(&self) -> bool {
        match self {
            CacheKey::Str(_) => false,
            CacheKey::List { persistent, .. } => *persistent,
        }
    }

    /// `JSON.stringify(queryKey)`.
    pub fn to_json(&self) -> String {
        match self {
            CacheKey::Str(value) => serde_json::Value::String(value.clone()).to_string(),
            CacheKey::List { items, .. } => KeyValue::Array(items.clone()).to_json(),
        }
    }

    /// The hash under this process' uid, see [`get_cache_hash`].
    pub fn hash(&self) -> String {
        get_cache_hash(self, process_uid())
    }
}

impl serde::Serialize for CacheKey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            CacheKey::Str(value) => serializer.serialize_str(value),
            CacheKey::List { items, .. } => {
                serde_json::Value::from(&KeyValue::Array(items.clone())).serialize(serializer)
            }
        }
    }
}

impl<'de> serde::Deserialize<'de> for CacheKey {
    /// The `persistent` flag is not part of the serialization and is lost here, exactly
    /// as it is in Node where it lives on the array object rather than in its JSON.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(match value {
            serde_json::Value::String(value) => CacheKey::Str(value),
            serde_json::Value::Array(items) => {
                CacheKey::list(items.iter().map(KeyValue::from).collect())
            }
            other => CacheKey::list(vec![KeyValue::from(&other)]),
        })
    }
}

/// The per process uuid v4 that suffixes persistent keys (`getProcessUid` of
/// `@cubejs-backend/shared`).
pub fn process_uid() -> &'static str {
    static PROCESS_UID: OnceLock<String> = OnceLock::new();

    PROCESS_UID.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

fn md5_hex(payload: &str) -> String {
    let digest = Md5::digest(payload.as_bytes());
    let mut out = String::with_capacity(32);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Returns the query hash of `query_key`.
///
/// Port of `getCacheHash` (`QO/utils.ts:15-33`):
/// * a string shorter than 256 characters is its own hash,
/// * anything else hashes as `md5(JSON.stringify(key))` — note that a long string hashes
///   over its *quoted* JSON form,
/// * a key flagged `persistent` gets a `@<processUid>` suffix so that only the process
///   which owns the stream picks it up.
pub fn get_cache_hash(query_key: &CacheKey, process_uid: &str) -> String {
    if let CacheKey::Str(value) = query_key {
        // `String::len` is bytes while `String.prototype.length` is UTF-16 code units.
        if value.chars().map(char::len_utf16).sum::<usize>() < 256 {
            return value.clone();
        }
    }

    let hash = md5_hex(&query_key.to_json());

    if query_key.is_persistent() {
        format!("{hash}@{process_uid}")
    } else {
        hash
    }
}

/// Everything `QueryCache.queryCacheKey` reads off a query body.
#[derive(Clone, Debug, Default)]
pub struct QueryCacheKeyInput {
    /// `queryBody.query` — absent for a build only request, where it serializes as `null`.
    pub query: Option<String>,
    /// `queryBody.values` — absent serializes as `null`.
    pub values: Option<Vec<String>>,
    /// `queryBody.preAggregations.map(p => p.loadSql)`.
    pub pre_aggregation_load_sql: Vec<KeyValue>,
    /// `queryBody.invalidate`, appended as a fourth element when present.
    pub invalidate: Option<KeyValue>,
    /// Carried on the key object, never serialized.
    pub persistent: bool,
}

/// `QueryCache.queryCacheKey(queryBody)` = `[query, values, preAggregations.map(p => p.loadSql)]`
/// plus `invalidate` when the body carries one (`QO/QueryCache.ts:469-481`).
pub fn query_cache_key(input: &QueryCacheKeyInput) -> CacheKey {
    let mut items = vec![
        input
            .query
            .as_ref()
            .map(|query| KeyValue::Str(query.clone()))
            .unwrap_or(KeyValue::Null),
        input
            .values
            .as_ref()
            .map(|values| KeyValue::strings(values))
            .unwrap_or(KeyValue::Null),
        KeyValue::Array(input.pre_aggregation_load_sql.clone()),
    ];

    if let Some(invalidate) = &input.invalidate {
        items.push(invalidate.clone());
    }

    CacheKey::List {
        items,
        persistent: input.persistent,
    }
}

/// `QueryCache.refreshKeyIdentity(sqlQuery, dataSource)` = `[sql, params, !!external, dataSource || 'default']`
/// (`QO/QueryCache.ts:490-498`).
pub fn refresh_key_identity(
    sql: &str,
    params: &[String],
    external: bool,
    data_source: Option<&str>,
) -> CacheKey {
    let data_source = match data_source {
        Some(value) if !value.is_empty() => value,
        _ => "default",
    };

    CacheKey::list(vec![
        KeyValue::Str(sql.to_string()),
        KeyValue::strings(params),
        KeyValue::Bool(external),
        KeyValue::Str(data_source.to_string()),
    ])
}

/// `QueryCache.getKey(catalog, key)` = `` `${cachePrefix}#${catalog}:${key}` ``.
pub fn cache_key_string(cache_prefix: &str, catalog: &str, key: &str) -> String {
    format!("{cache_prefix}#{catalog}:{key}")
}

/// The cache driver key of a query result: `` `${cachePrefix}#SQL_QUERY_RESULT:${getCacheHash(key)}` ``.
pub fn query_result_cache_key(cache_prefix: &str, key: &CacheKey, process_uid: &str) -> String {
    cache_key_string(
        cache_prefix,
        SQL_QUERY_RESULT,
        &get_cache_hash(key, process_uid),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The uid the golden vectors below were generated with.
    const PUID: &str = "00000000-0000-4000-8000-000000000000";

    fn hash(key: &CacheKey) -> String {
        get_cache_hash(key, PUID)
    }

    /// Every expectation in this module was produced by running the repository's own
    /// implementation under Node as an oracle, from the repository root:
    ///
    /// ```text
    /// node -e "
    ///   const d = './packages/cubejs-query-orchestrator/dist/src/orchestrator/';
    ///   const { getCacheHash } = require(d + 'utils');
    ///   const { QueryCache } = require(d + 'QueryCache');
    ///   const key = QueryCache.queryCacheKey({ query: 'SELECT 1', values: [] });
    ///   console.log(JSON.stringify(key), getCacheHash(key, '00000000-0000-4000-8000-000000000000'));
    /// "
    /// ```
    #[test]
    fn golden_string_keys() {
        assert_eq!(hash(&CacheKey::string("string")), "string");
        // 255 characters still take the shortcut, 256 do not.
        assert_eq!(hash(&CacheKey::string("y".repeat(255))), "y".repeat(255));
        assert_eq!(
            hash(&CacheKey::string("z".repeat(256))),
            "121e5b2b978f0f5780814f076867180d"
        );
        assert_eq!(
            hash(&CacheKey::string("x".repeat(300))),
            "ac1962b3d00eb6e41a0ddead5790c7aa"
        );
    }

    #[test]
    fn golden_long_string_hashes_over_its_quoted_json() {
        let key = CacheKey::string("z".repeat(256));
        assert_eq!(key.to_json(), format!("\"{}\"", "z".repeat(256)));
        assert_eq!(hash(&key), md5_hex(&key.to_json()));
    }

    #[test]
    fn golden_sql_tuple_keys() {
        let key = CacheKey::sql("select * from", &[]);
        assert_eq!(key.to_json(), r#"["select * from",[]]"#);
        assert_eq!(hash(&key), "c592e0ebeceeea090daecef4a03ce9ce");

        let key = CacheKey::sql("SELECT 1 FROM t WHERE a = $1", &["a\"b\\c\nd".to_string()]);
        assert_eq!(
            key.to_json(),
            r#"["SELECT 1 FROM t WHERE a = $1",["a\"b\\c\nd"]]"#
        );
        assert_eq!(hash(&key), "e47996350f01e33686dd99c0c3ff216f");
    }

    #[test]
    fn golden_persistent_suffix() {
        let key = CacheKey::sql("select * from table", &[]);
        assert_eq!(hash(&key), "bc6c8a93a7dca1e97320577af064f4b5");
        assert_eq!(
            hash(&key.clone().with_persistent(false)),
            "bc6c8a93a7dca1e97320577af064f4b5"
        );
        assert_eq!(
            hash(&key.with_persistent(true)),
            format!("bc6c8a93a7dca1e97320577af064f4b5@{PUID}")
        );
    }

    #[test]
    fn golden_query_cache_keys() {
        let minimal = query_cache_key(&QueryCacheKeyInput {
            query: Some("SELECT 1".into()),
            values: Some(vec![]),
            ..Default::default()
        });
        assert_eq!(minimal.to_json(), r#"["SELECT 1",[],[]]"#);
        assert_eq!(hash(&minimal), "64f50e6045e9b468b5be89010abb39f6");

        // `undefined` values serialize as `null`
        let no_values = query_cache_key(&QueryCacheKeyInput {
            query: Some("SELECT 1".into()),
            ..Default::default()
        });
        assert_eq!(no_values.to_json(), r#"["SELECT 1",null,[]]"#);
        assert_eq!(hash(&no_values), "33d1adbea0c7400b5d19783c4eefba90");

        let with_pre_aggs = query_cache_key(&QueryCacheKeyInput {
            query: Some("SELECT * FROM t".into()),
            values: Some(vec!["1".into(), "2".into()]),
            pre_aggregation_load_sql: vec![
                KeyValue::sql("CREATE TABLE pa AS SELECT 1", &["p1".to_string()]),
                KeyValue::sql("CREATE TABLE pb AS SELECT 2", &[]),
            ],
            ..Default::default()
        });
        assert_eq!(
            with_pre_aggs.to_json(),
            r#"["SELECT * FROM t",["1","2"],[["CREATE TABLE pa AS SELECT 1",["p1"]],["CREATE TABLE pb AS SELECT 2",[]]]]"#
        );
        assert_eq!(hash(&with_pre_aggs), "6c683940b311791324826e98a685b7ec");

        let with_invalidate = query_cache_key(&QueryCacheKeyInput {
            query: Some("SELECT * FROM t".into()),
            values: Some(vec!["1".into()]),
            invalidate: Some(KeyValue::Array(vec![
                KeyValue::Str("SELECT MAX(x) FROM t".into()),
                KeyValue::Array(vec![]),
                KeyValue::Bool(false),
                KeyValue::Str("default".into()),
            ])),
            ..Default::default()
        });
        assert_eq!(
            with_invalidate.to_json(),
            r#"["SELECT * FROM t",["1"],[],["SELECT MAX(x) FROM t",[],false,"default"]]"#
        );
        assert_eq!(hash(&with_invalidate), "11f6faffba4f081c043e2711f8d30a34");

        let persistent = query_cache_key(&QueryCacheKeyInput {
            query: Some("SELECT 1".into()),
            values: Some(vec![]),
            persistent: true,
            ..Default::default()
        });
        assert_eq!(
            hash(&persistent),
            format!("64f50e6045e9b468b5be89010abb39f6@{PUID}")
        );
    }

    #[test]
    fn golden_refresh_key_identity() {
        let key = refresh_key_identity("SELECT MAX(id) FROM t", &[], false, None);
        assert_eq!(
            key.to_json(),
            r#"["SELECT MAX(id) FROM t",[],false,"default"]"#
        );
        assert_eq!(hash(&key), "b0c65a0cacfb9f6319db52ed0d74a998");

        let key = refresh_key_identity(
            "SELECT MAX(id) FROM t",
            &["7".to_string()],
            true,
            Some("ds1"),
        );
        assert_eq!(hash(&key), "de51317da48e6fbd55a86f3d9bf475c1");

        // an absent data source and an explicit "default" are the same key
        assert_eq!(
            hash(&refresh_key_identity(
                "SELECT 1",
                &[],
                false,
                Some("default")
            )),
            hash(&refresh_key_identity("SELECT 1", &[], false, None))
        );
        assert_eq!(
            hash(&refresh_key_identity("SELECT 1", &[], false, None)),
            "f52e58fca6cc116059fcd253707b8135"
        );
    }

    #[test]
    fn golden_unicode_and_escaping() {
        let key = CacheKey::sql(
            "SELECT 'ünïcødé ☃' AS a",
            &["\"quoted\"".to_string(), "tab\there".to_string()],
        );
        assert_eq!(
            key.to_json(),
            "[\"SELECT 'ünïcødé ☃' AS a\",[\"\\\"quoted\\\"\",\"tab\\there\"]]"
        );
        assert_eq!(hash(&key), "95cfcec4cfa7cc9511dcfa17ca80660f");
    }

    #[test]
    fn golden_object_field_order_is_preserved() {
        let b_first = CacheKey::list(vec![KeyValue::Object(vec![
            ("b".into(), KeyValue::Int(2)),
            ("a".into(), KeyValue::Int(1)),
        ])]);
        let a_first = CacheKey::list(vec![KeyValue::Object(vec![
            ("a".into(), KeyValue::Int(1)),
            ("b".into(), KeyValue::Int(2)),
        ])]);

        assert_eq!(b_first.to_json(), r#"[{"b":2,"a":1}]"#);
        assert_eq!(hash(&b_first), "8a327e571dd2aed97cf6d0b5b66d5b17");
        assert_eq!(a_first.to_json(), r#"[{"a":1,"b":2}]"#);
        assert_eq!(hash(&a_first), "f4ec9afe5adeab0e29967a1f205ced34");
    }

    #[test]
    fn golden_nested_options_object() {
        let key = CacheKey::list(vec![
            KeyValue::Str("SELECT 1".into()),
            KeyValue::Array(vec![]),
            KeyValue::Object(vec![
                ("external".into(), KeyValue::Bool(false)),
                ("renewalThreshold".into(), KeyValue::Int(120)),
                ("incremental".into(), KeyValue::Bool(true)),
            ]),
        ]);
        assert_eq!(
            key.to_json(),
            r#"["SELECT 1",[],{"external":false,"renewalThreshold":120,"incremental":true}]"#
        );
        assert_eq!(hash(&key), "b8c23ca1f8e633d40609228fd6c51e99");
    }

    #[test]
    fn golden_empty_array() {
        let key = CacheKey::list(vec![]);
        assert_eq!(key.to_json(), "[]");
        assert_eq!(hash(&key), "d751713988987e9331980363e24189ce");
    }

    #[test]
    fn cache_key_strings() {
        let key = query_cache_key(&QueryCacheKeyInput {
            query: Some("SELECT 1".into()),
            values: Some(vec![]),
            ..Default::default()
        });

        assert_eq!(
            query_result_cache_key("prefix", &key, PUID),
            "prefix#SQL_QUERY_RESULT:64f50e6045e9b468b5be89010abb39f6"
        );
        assert_eq!(cache_key_string("p", "CATALOG", "k"), "p#CATALOG:k");
    }

    #[test]
    fn process_uid_is_a_stable_uuid() {
        let uid = process_uid().to_string();

        assert_eq!(uid.len(), 36);
        assert_eq!(uid, process_uid());
        assert!(uid
            .chars()
            .all(|c| c == '-' || c.is_ascii_digit() || ('a'..='f').contains(&c)));
        assert_eq!(
            uid.split('-').map(str::len).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
    }

    #[test]
    fn serde_round_trip_keeps_the_key_shape() {
        let key = CacheKey::sql("SELECT 1", &["a".to_string()]);
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(json, r#"["SELECT 1",["a"]]"#);
        assert_eq!(serde_json::from_str::<CacheKey>(&json).unwrap(), key);

        let key = CacheKey::string("plain");
        let json = serde_json::to_string(&key).unwrap();
        assert_eq!(json, r#""plain""#);
        assert_eq!(serde_json::from_str::<CacheKey>(&json).unwrap(), key);
    }
}
