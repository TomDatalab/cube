//! The value types the orchestrator exchanges with the API gateway and the data model.
//!
//! Port of the `QueryBody`, `QueryWithParams`, `PreAggregationDescription` and
//! `LoadPreAggregationResult` types of `QO/QueryCache.ts` and `QO/PreAggregations.ts`.

use std::collections::BTreeMap;

use cubecache::KeyValue;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::refresh_key::LocalRefreshKeyDescriptor;

/// `CacheMode` (`packages/cubejs-backend-shared/src/shared-types.ts:22`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CacheMode {
    StaleIfSlow,
    StaleWhileRevalidate,
    MustRevalidate,
    NoCache,
}

/// The options element of a refresh key query (`QueryOptions` of `QO/QueryCache.ts:69-76`).
///
/// The field order is the declaration order of the TypeScript type, because the options
/// object takes part in the `renewalKey` hash through `JSON.stringify`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshKeyQueryOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renewal_threshold: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_window_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renewal_threshold_outside_update_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incremental: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_refresh_key: Option<LocalRefreshKeyDescriptor>,
}

impl RefreshKeyQueryOptions {
    fn to_key_value(&self) -> KeyValue {
        let mut fields: Vec<(String, KeyValue)> = Vec::new();

        if let Some(external) = self.external {
            fields.push(("external".into(), KeyValue::Bool(external)));
        }
        if let Some(value) = self.renewal_threshold {
            fields.push(("renewalThreshold".into(), KeyValue::Int(value as i64)));
        }
        if let Some(value) = self.update_window_seconds {
            fields.push(("updateWindowSeconds".into(), KeyValue::Int(value as i64)));
        }
        if let Some(value) = self.renewal_threshold_outside_update_window {
            fields.push((
                "renewalThresholdOutsideUpdateWindow".into(),
                KeyValue::Int(value as i64),
            ));
        }
        if let Some(value) = self.incremental {
            fields.push(("incremental".into(), KeyValue::Bool(value)));
        }
        if let Some(value) = &self.local_refresh_key {
            fields.push(("localRefreshKey".into(), value.to_key_value()));
        }

        KeyValue::Object(fields)
    }
}

/// `QueryWithParams = [sql, params, options?]` (`QO/QueryCache.ts:78-82`).
///
/// It serializes as the JavaScript tuple so that a value of it hashes identically inside
/// a `renewalKey` or a content version.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueryWithParams {
    pub sql: String,
    pub params: Vec<String>,
    pub options: Option<RefreshKeyQueryOptions>,
}

impl QueryWithParams {
    pub fn new(sql: impl Into<String>, params: Vec<String>) -> Self {
        Self {
            sql: sql.into(),
            params,
            options: None,
        }
    }

    #[must_use]
    pub fn with_options(mut self, options: RefreshKeyQueryOptions) -> Self {
        self.options = Some(options);
        self
    }

    /// `!!options?.external`
    pub fn is_external(&self) -> bool {
        self.options
            .as_ref()
            .and_then(|options| options.external)
            .unwrap_or(false)
    }

    pub fn to_key_value(&self) -> KeyValue {
        let mut items = vec![
            KeyValue::Str(self.sql.clone()),
            KeyValue::strings(&self.params),
        ];

        if let Some(options) = &self.options {
            items.push(options.to_key_value());
        }

        KeyValue::Array(items)
    }
}

impl Serialize for QueryWithParams {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;

        let len = if self.options.is_some() { 3 } else { 2 };
        let mut seq = serializer.serialize_seq(Some(len))?;
        seq.serialize_element(&self.sql)?;
        seq.serialize_element(&self.params)?;
        if let Some(options) = &self.options {
            seq.serialize_element(options)?;
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for QueryWithParams {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Tuple2(String, Vec<String>),
            Tuple3(String, Vec<String>, Option<RefreshKeyQueryOptions>),
            Sql(String),
        }

        Ok(match Repr::deserialize(deserializer)? {
            Repr::Tuple3(sql, params, options) => QueryWithParams {
                sql,
                params,
                options,
            },
            Repr::Tuple2(sql, params) => QueryWithParams {
                sql,
                params,
                options: None,
            },
            Repr::Sql(sql) => QueryWithParams {
                sql,
                params: Vec::new(),
                options: None,
            },
        })
    }
}

/// `queryBody.cacheKeyQueries`, which the data model writes either as a bare array or as
/// `{ queries, renewalThreshold }` (`QueryCache.cacheKeyQueriesFrom`, `:463-467`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CacheKeyQueries {
    List(Vec<QueryWithParams>),
    #[serde(rename_all = "camelCase")]
    WithThreshold {
        #[serde(default)]
        queries: Vec<QueryWithParams>,
        #[serde(default)]
        renewal_threshold: Option<u64>,
    },
}

impl CacheKeyQueries {
    pub fn queries(&self) -> &[QueryWithParams] {
        match self {
            CacheKeyQueries::List(queries) => queries,
            CacheKeyQueries::WithThreshold { queries, .. } => queries,
        }
    }

    pub fn renewal_threshold(&self) -> Option<u64> {
        match self {
            CacheKeyQueries::List(_) => None,
            CacheKeyQueries::WithThreshold {
                renewal_threshold, ..
            } => *renewal_threshold,
        }
    }
}

/// A pre-aggregation description as the data model compiler emits it
/// (`PreAggregationDescription`, `QO/PreAggregations.ts:160-216`). Only the fields this
/// port reads are modelled; everything else survives in [`PreAggregationDescription::extra`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreAggregationDescription {
    pub table_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_aggregation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_aggregations_schema: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_sql: Option<QueryWithParams>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structure_version_load_sql: Option<QueryWithParams>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql: Option<QueryWithParams>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalidate_key_queries: Option<Vec<QueryWithParams>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition_invalidate_key_queries: Option<Vec<QueryWithParams>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexes_sql: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_column_types: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_offset: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unique_key_columns: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<bool>,
    /// `preAggregation.readOnly` — forces `refreshReadOnlyExternalStrategy` even when the
    /// driver would allow writing a temporary table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition_granularity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,

    // ------------------------------------------------------------------
    // Partitioning (`PreAggregationPartitionRangeLoader`)
    // ------------------------------------------------------------------
    /// The `[min, max]` queries the build range is read from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_aggregation_start_end_queries: Option<Vec<QueryWithParams>>,
    /// The date range of the query this pre-aggregation is serving, already localized by
    /// `BaseFilter.formatFromDate/formatToDate`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matched_time_dimension_date_range: Option<Vec<String>>,
    /// Digits of the sub-second part every generated timestamp carries. Absent means 3;
    /// Node refuses the build instead, but every description the compiler emits sets it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp_precision: Option<u32>,
    /// The `moment` format the data source expects a timestamp parameter in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp_format: Option<String>,
    /// How long after a partition's upper bound the partition may still change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_window_seconds: Option<u64>,
    /// Set on the descriptions `partitionPreAggregationDescription` produces, so that a
    /// partition is never expanded a second time.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub expanded_partition: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_range_start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_range_end: Option<String>,
    /// Passed to the external store so a partition that can still change is not sealed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seal_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_sql: Option<QueryWithParams>,
    /// `suffix -> { dateRange }`: one description serving several usages, each of which may
    /// only need a subset of the partitions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_mapping: Option<serde_json::Map<String, Value>>,

    // ------------------------------------------------------------------
    // External store table creation
    // ------------------------------------------------------------------
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregations_columns: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_table_indexes: Option<Value>,

    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl PreAggregationDescription {
    /// `preAggregation.dataSource || 'default'`
    pub fn data_source(&self) -> &str {
        match self.data_source.as_deref() {
            Some(value) if !value.is_empty() => value,
            _ => "default",
        }
    }

    /// The schema half of `schema.table`.
    pub fn schema(&self) -> &str {
        self.table_name.split('.').next().unwrap_or("")
    }

    /// The table half of `schema.table`, which is what `tablePrefixes` is built from.
    pub fn table_prefix(&self) -> &str {
        let mut parts = self.table_name.splitn(2, '.');
        parts.next();
        parts.next().unwrap_or("")
    }

    /// `preAggregation.timezone`. Node hands `undefined` straight to `moment.tz.zone()` and
    /// fails with `Unknown timezone: undefined`; a description that reaches a partition
    /// build always carries one, so an absent value is read as UTC here.
    pub fn timezone(&self) -> &str {
        match self.timezone.as_deref() {
            Some(value) if !value.is_empty() => value,
            _ => "UTC",
        }
    }

    /// `preAggregation.timestampPrecision`, defaulting to the three digits every
    /// `timeSeries` call uses when the option object is left out entirely.
    pub fn timestamp_precision(&self) -> u32 {
        self.timestamp_precision.unwrap_or(3)
    }

    /// `preAggregation.matchedTimeDimensionDateRange` as the pair the intersection needs.
    pub fn matched_time_dimension_date_range(&self) -> Option<(String, String)> {
        let range = self.matched_time_dimension_date_range.as_ref()?;

        match range.as_slice() {
            [from, to] => Some((from.clone(), to.clone())),
            _ => None,
        }
    }
}

/// `LoadPreAggregationResult` (`QO/PreAggregations.ts:118-135`), stamped by
/// `loadAllPreAggregationsIfNeeded` with the identity of the description it came from.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadPreAggregationResult {
    pub target_table_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_key_values: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_updated_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_range_end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_aggregation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_multi_table_union: bool,
    /// One description that serves several table names, `tableName + suffix`
    /// (`QO/PreAggregations.ts:613-625`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub usage_target_table_names: BTreeMap<String, String>,
}

/// `PreAggTableToTempTable = [tableName, LoadPreAggregationResult]`.
pub type PreAggTableToTempTable = (String, LoadPreAggregationResult);

/// One entry of `fetchQuery`'s `usedPreAggregations`
/// (`QO/QueryOrchestrator.ts:225-238`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsedPreAggregation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_table_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_key_values: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_updated_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pre_aggregation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
}

impl From<&LoadPreAggregationResult> for UsedPreAggregation {
    fn from(result: &LoadPreAggregationResult) -> Self {
        Self {
            target_table_name: Some(result.target_table_name.clone()),
            refresh_key_values: result.refresh_key_values.clone(),
            last_updated_at: result.last_updated_at,
            pre_aggregation_id: result.pre_aggregation_id.clone(),
            r#type: result.r#type.clone(),
        }
    }
}

/// The request `fetchQuery` receives (`QueryBody`, `QO/QueryCache.ts:101-122`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryBody {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_mode: Option<CacheMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external: Option<bool>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub persistent: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_job: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub force_no_cache: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub scheduled_refresh: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub load_refresh_keys_only: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub force_build_pre_aggregations: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_priority: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expire_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_key_queries: Option<CacheKeyQueries>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalidate: Option<QueryWithParams>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pre_aggregations: Vec<PreAggregationDescription>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias_name_to_member: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl QueryBody {
    /// `queryBody.dataSource` with the `getQueue` default applied.
    pub fn data_source(&self) -> &str {
        match self.data_source.as_deref() {
            Some(value) if !value.is_empty() => value,
            _ => "default",
        }
    }

    /// `queryBody.external` as the boolean the queue routing reads.
    pub fn is_external(&self) -> bool {
        self.external.unwrap_or(false)
    }

    /// `queryBody.expireSecs || 24 * 3600` (`QueryCache.getExpireSecs`, `:459-461`).
    pub fn expire_secs(&self) -> u64 {
        match self.expire_secs {
            Some(value) if value > 0 => value,
            _ => 24 * 3600,
        }
    }

    /// `cacheKeyQueriesFrom` (`:463-467`).
    pub fn cache_key_queries(&self) -> &[QueryWithParams] {
        self.cache_key_queries
            .as_ref()
            .map(CacheKeyQueries::queries)
            .unwrap_or(&[])
    }

    pub fn renewal_threshold(&self) -> Option<u64> {
        self.cache_key_queries
            .as_ref()
            .and_then(CacheKeyQueries::renewal_threshold)
    }
}

/// Deserializes a `QueryBody` from the loosely typed JSON the gateway hands over.
pub fn query_body_from_value(value: &Value) -> Result<QueryBody, serde_json::Error> {
    serde_json::from_value(value.clone())
}

/// `Number.isInteger(queryBody.queuePriority) ? queryBody.queuePriority : Interactive`
/// (`QO/QueryCache.ts:293-297`).
pub fn resolve_queue_priority(queue_priority: Option<i32>) -> i32 {
    queue_priority.unwrap_or(cubequeue::QueuePriority::Interactive.value())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn query_with_params_serializes_as_a_tuple() {
        let query = QueryWithParams::new("SELECT 1", vec!["a".into()]);
        assert_eq!(
            serde_json::to_string(&query).unwrap(),
            r#"["SELECT 1",["a"]]"#
        );

        let with_options = query.clone().with_options(RefreshKeyQueryOptions {
            external: Some(true),
            renewal_threshold: Some(120),
            ..Default::default()
        });
        assert_eq!(
            serde_json::to_string(&with_options).unwrap(),
            r#"["SELECT 1",["a"],{"external":true,"renewalThreshold":120}]"#
        );

        assert_eq!(
            serde_json::from_str::<QueryWithParams>(r#"["SELECT 1",["a"]]"#).unwrap(),
            query
        );
        assert_eq!(
            serde_json::from_str::<QueryWithParams>(
                r#"["SELECT 1",["a"],{"external":true,"renewalThreshold":120}]"#
            )
            .unwrap(),
            with_options
        );
    }

    #[test]
    fn query_with_params_key_value_matches_json_stringify() {
        let query = QueryWithParams::new("SELECT 1", vec![]).with_options(RefreshKeyQueryOptions {
            external: Some(false),
            renewal_threshold: Some(120),
            incremental: Some(true),
            ..Default::default()
        });

        assert_eq!(
            query.to_key_value().to_json(),
            r#"["SELECT 1",[],{"external":false,"renewalThreshold":120,"incremental":true}]"#
        );
    }

    #[test]
    fn cache_key_queries_accepts_both_spellings() {
        let list: CacheKeyQueries =
            serde_json::from_value(json!([["SELECT MAX(id) FROM t", []]])).unwrap();
        assert_eq!(list.queries().len(), 1);
        assert_eq!(list.renewal_threshold(), None);

        let object: CacheKeyQueries = serde_json::from_value(json!({
            "queries": [["SELECT MAX(id) FROM t", []]],
            "renewalThreshold": 42,
        }))
        .unwrap();
        assert_eq!(object.queries().len(), 1);
        assert_eq!(object.renewal_threshold(), Some(42));
    }

    #[test]
    fn query_body_defaults() {
        let body: QueryBody = serde_json::from_value(json!({ "query": "SELECT 1" })).unwrap();

        assert_eq!(body.data_source(), "default");
        assert_eq!(body.expire_secs(), 86400);
        assert!(body.cache_key_queries().is_empty());
        assert_eq!(body.renewal_threshold(), None);
        assert!(!body.is_external());
        assert_eq!(resolve_queue_priority(body.queue_priority), 10);
        assert_eq!(resolve_queue_priority(Some(-1)), -1);
    }

    #[test]
    fn pre_aggregation_description_splits_its_table_name() {
        let description = PreAggregationDescription {
            table_name: "stb_pre_aggregations.orders_main".to_string(),
            ..Default::default()
        };

        assert_eq!(description.schema(), "stb_pre_aggregations");
        assert_eq!(description.table_prefix(), "orders_main");
        assert_eq!(description.data_source(), "default");
    }

    #[test]
    fn unknown_query_body_fields_survive() {
        let body: QueryBody =
            serde_json::from_value(json!({ "query": "SELECT 1", "somethingElse": 7 })).unwrap();

        assert_eq!(body.extra.get("somethingElse"), Some(&json!(7)));
    }
}
