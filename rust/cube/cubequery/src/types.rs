//! The REST API query model: the JSON shapes accepted from clients (`Query`),
//! the normalized shape handed to the planner (`NormalizedQuery`) and the
//! supporting enums. Field names are the camelCase names of the JSON API.
//!
//! Deserialization accepts the same lenient input shapes the Joi schema in
//! `query.js` accepts (`order` as an object or an array of pairs,
//! `dateRange` as a string or an array, `member`/`dimension` aliases,
//! boolean `and`/`or` filter groups, ...).

use std::fmt;

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Number;

/// Query type of a request (`QueryType` enum in `types/enums.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum QueryType {
    RegularQuery,
    CompareDateRangeQuery,
    BlendingQuery,
}

impl QueryType {
    pub fn as_str(self) -> &'static str {
        match self {
            QueryType::RegularQuery => "regularQuery",
            QueryType::CompareDateRangeQuery => "compareDateRangeQuery",
            QueryType::BlendingQuery => "blendingQuery",
        }
    }
}

impl fmt::Display for QueryType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Result dataset format (`ResultType` enum / `responseFormat`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResultType {
    #[default]
    Default,
    Compact,
    Columnar,
}

impl ResultType {
    pub fn as_str(self) -> &'static str {
        match self {
            ResultType::Default => "default",
            ResultType::Compact => "compact",
            ResultType::Columnar => "columnar",
        }
    }
}

/// API type of a request (`ApiType` in `types/strings.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApiType {
    Sql,
    Graphql,
    Rest,
    Ws,
    Stream,
}

/// Cache mode (`CacheMode` in `@cubejs-backend/shared`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CacheMode {
    #[default]
    StaleIfSlow,
    StaleWhileRevalidate,
    MustRevalidate,
    NoCache,
}

impl CacheMode {
    pub fn as_str(self) -> &'static str {
        match self {
            CacheMode::StaleIfSlow => "stale-if-slow",
            CacheMode::StaleWhileRevalidate => "stale-while-revalidate",
            CacheMode::MustRevalidate => "must-revalidate",
            CacheMode::NoCache => "no-cache",
        }
    }
}

/// Order direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OrderDirection {
    Asc,
    Desc,
}

/// Filter operators supported by the REST API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FilterOperator {
    Equals,
    NotEquals,
    Contains,
    NotContains,
    StartsWith,
    NotStartsWith,
    EndsWith,
    NotEndsWith,
    In,
    NotIn,
    Gt,
    Gte,
    Lt,
    Lte,
    Set,
    NotSet,
    InDateRange,
    NotInDateRange,
    OnTheDate,
    BeforeDate,
    BeforeOrOnDate,
    AfterDate,
    AfterOrOnDate,
    MeasureFilter,
}

impl FilterOperator {
    pub fn as_str(self) -> &'static str {
        match self {
            FilterOperator::Equals => "equals",
            FilterOperator::NotEquals => "notEquals",
            FilterOperator::Contains => "contains",
            FilterOperator::NotContains => "notContains",
            FilterOperator::StartsWith => "startsWith",
            FilterOperator::NotStartsWith => "notStartsWith",
            FilterOperator::EndsWith => "endsWith",
            FilterOperator::NotEndsWith => "notEndsWith",
            FilterOperator::In => "in",
            FilterOperator::NotIn => "notIn",
            FilterOperator::Gt => "gt",
            FilterOperator::Gte => "gte",
            FilterOperator::Lt => "lt",
            FilterOperator::Lte => "lte",
            FilterOperator::Set => "set",
            FilterOperator::NotSet => "notSet",
            FilterOperator::InDateRange => "inDateRange",
            FilterOperator::NotInDateRange => "notInDateRange",
            FilterOperator::OnTheDate => "onTheDate",
            FilterOperator::BeforeDate => "beforeDate",
            FilterOperator::BeforeOrOnDate => "beforeOrOnDate",
            FilterOperator::AfterDate => "afterDate",
            FilterOperator::AfterOrOnDate => "afterOrOnDate",
            FilterOperator::MeasureFilter => "measureFilter",
        }
    }

    /// Operators that do not need `values` (`set`, `notSet`, `measureFilter`).
    pub fn allows_empty_values(self) -> bool {
        matches!(
            self,
            FilterOperator::Set | FilterOperator::NotSet | FilterOperator::MeasureFilter
        )
    }
}

/// Join type of a subquery join.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum JoinType {
    Left,
    Inner,
}

/// Grouping set type of a member expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GroupType {
    Rollup,
    Cube,
}

/// Grouping set of a parsed member expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GroupingSet {
    pub group_type: GroupType,
    pub id: Number,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sub_id: Option<Number>,
}

/// The `type: "PatchMeasure"` tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum PatchMeasureTag {
    #[default]
    PatchMeasure,
}

/// A parsed `PatchMeasure` expression: each filter is a parsed SQL function
/// (`[...cubeParams, "return `sql`"]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PatchMeasureExpression {
    #[serde(rename = "type")]
    pub kind: PatchMeasureTag,
    pub source_measure: String,
    pub replace_aggregation_type: Option<String>,
    pub add_filters: Vec<Vec<String>>,
}

/// The `expression` of a parsed member expression: either a parsed SQL
/// function (`[...cubeParams, "return `sql`"]`) or a `PatchMeasure`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum MemberExpressionBody {
    Sql(Vec<String>),
    PatchMeasure(PatchMeasureExpression),
}

impl<'de> Deserialize<'de> for MemberExpressionBody {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct BodyVisitor;

        impl<'de> Visitor<'de> for BodyVisitor {
            type Value = MemberExpressionBody;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an array of strings or a PatchMeasure object")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element::<String>()? {
                    items.push(item);
                }
                Ok(MemberExpressionBody::Sql(items))
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                Deserialize::deserialize(de::value::MapAccessDeserializer::new(map))
                    .map(MemberExpressionBody::PatchMeasure)
            }
        }

        deserializer.deserialize_any(BodyVisitor)
    }
}

/// A member expression after `parseMemberExpressionsInQuery`
/// (`ParsedMemberExpression` in `types/query.ts`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ParsedMemberExpression {
    pub expression: MemberExpressionBody,
    pub cube_name: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grouping_set: Option<GroupingSet>,
}

/// A measure, dimension, segment or subquery join target: either a member
/// name (`Cube.member`, optionally `Cube.member.granularity` for dimensions)
/// or a parsed member expression object.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum QueryMember {
    Name(String),
    Expression(ParsedMemberExpression),
}

impl QueryMember {
    pub fn as_name(&self) -> Option<&str> {
        match self {
            QueryMember::Name(name) => Some(name),
            QueryMember::Expression(_) => None,
        }
    }
}

impl From<&str> for QueryMember {
    fn from(name: &str) -> Self {
        QueryMember::Name(name.to_string())
    }
}

impl From<String> for QueryMember {
    fn from(name: String) -> Self {
        QueryMember::Name(name)
    }
}

impl<'de> Deserialize<'de> for QueryMember {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct MemberVisitor;

        impl<'de> Visitor<'de> for MemberVisitor {
            type Value = QueryMember;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a member name or a member expression object")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(QueryMember::Name(v.to_string()))
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(QueryMember::Name(v))
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                Deserialize::deserialize(de::value::MapAccessDeserializer::new(map))
                    .map(QueryMember::Expression)
            }
        }

        deserializer.deserialize_any(MemberVisitor)
    }
}

/// A filter value as accepted by the API: string, number, boolean or null.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum FilterValue {
    String(String),
    Number(Number),
    Bool(bool),
    Null,
}

impl FilterValue {
    /// JavaScript `v != null ? v.toString() : v`.
    pub fn to_js_string(&self) -> Option<String> {
        match self {
            FilterValue::String(s) => Some(s.clone()),
            FilterValue::Number(n) => Some(js_number_to_string(n)),
            FilterValue::Bool(b) => Some(b.to_string()),
            FilterValue::Null => None,
        }
    }
}

impl From<&str> for FilterValue {
    fn from(value: &str) -> Self {
        FilterValue::String(value.to_string())
    }
}

/// `Number.prototype.toString()` for the values JSON can carry.
pub fn js_number_to_string(n: &Number) -> String {
    if let Some(i) = n.as_i64() {
        return i.to_string();
    }
    if let Some(u) = n.as_u64() {
        return u.to_string();
    }
    let f = n.as_f64().unwrap_or(f64::NAN);
    if f.is_nan() {
        return "NaN".to_string();
    }
    if f.is_infinite() {
        return if f > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    if f == 0.0 {
        return "0".to_string();
    }
    let abs = f.abs();
    if abs >= 1e21 || abs < 1e-6 {
        let formatted = format!("{f:e}");
        return match formatted.split_once('e') {
            Some((mantissa, exp)) if !exp.starts_with('-') => format!("{mantissa}e+{exp}"),
            _ => formatted,
        };
    }
    if f.fract() == 0.0 {
        return format!("{f:.0}");
    }
    format!("{f}")
}

impl<'de> Deserialize<'de> for FilterValue {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ValueVisitor;

        impl<'de> Visitor<'de> for ValueVisitor {
            type Value = FilterValue;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a string, number, boolean or null")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(FilterValue::String(v.to_string()))
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(FilterValue::String(v))
            }

            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(FilterValue::Bool(v))
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(FilterValue::Number(v.into()))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(FilterValue::Number(v.into()))
            }

            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
                Number::from_f64(v)
                    .map(FilterValue::Number)
                    .ok_or_else(|| E::custom("non-finite number"))
            }

            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(FilterValue::Null)
            }

            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(FilterValue::Null)
            }
        }

        deserializer.deserialize_any(ValueVisitor)
    }
}

/// A single filter condition as sent by clients. Exactly one of `member` and
/// `dimension` (the legacy alias) must be present.
///
/// There is no `Default`: `operator` is required by the Joi schema, so an
/// empty filter is not a meaningful value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilterLeaf {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimension: Option<String>,
    pub operator: FilterOperator,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<FilterValue>>,
}

/// A filter: a single condition or a boolean `and`/`or` group.
#[derive(Debug, Clone, PartialEq)]
pub enum QueryFilter {
    Leaf(FilterLeaf),
    Or(Vec<QueryFilter>),
    And(Vec<QueryFilter>),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawFilter {
    #[serde(default)]
    or: Option<Vec<QueryFilter>>,
    #[serde(default)]
    and: Option<Vec<QueryFilter>>,
    #[serde(default)]
    member: Option<String>,
    #[serde(default)]
    dimension: Option<String>,
    #[serde(default)]
    operator: Option<FilterOperator>,
    #[serde(default)]
    values: Option<Vec<FilterValue>>,
}

impl<'de> Deserialize<'de> for QueryFilter {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawFilter::deserialize(deserializer)?;
        let is_group = raw.or.is_some() || raw.and.is_some();
        let has_leaf_keys = raw.member.is_some()
            || raw.dimension.is_some()
            || raw.operator.is_some()
            || raw.values.is_some();
        if is_group && has_leaf_keys {
            return Err(de::Error::custom("does not match any of the allowed types"));
        }
        match (raw.or, raw.and) {
            (Some(_), Some(_)) => Err(de::Error::custom(
                "contains a conflict between exclusive peers [or, and]",
            )),
            (Some(or), None) => Ok(QueryFilter::Or(or)),
            (None, Some(and)) => Ok(QueryFilter::And(and)),
            (None, None) => {
                let operator = raw
                    .operator
                    .ok_or_else(|| de::Error::custom("\"operator\" is required"))?;
                Ok(QueryFilter::Leaf(FilterLeaf {
                    member: raw.member,
                    dimension: raw.dimension,
                    operator,
                    values: raw.values,
                }))
            }
        }
    }
}

fn serialize_group<S: Serializer, T: Serialize>(
    serializer: S,
    key: &str,
    items: &[T],
) -> Result<S::Ok, S::Error> {
    let mut map = serializer.serialize_map(Some(1))?;
    map.serialize_entry(key, items)?;
    map.end()
}

impl Serialize for QueryFilter {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            QueryFilter::Leaf(leaf) => leaf.serialize(serializer),
            QueryFilter::Or(items) => serialize_group(serializer, "or", items),
            QueryFilter::And(items) => serialize_group(serializer, "and", items),
        }
    }
}

/// A `dateRange`: a single string (a relative range like `last 7 days` or a
/// single day) or an array of one or two dates.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum DateRange {
    Single(String),
    Range(Vec<String>),
}

impl From<&str> for DateRange {
    fn from(value: &str) -> Self {
        DateRange::Single(value.to_string())
    }
}

impl From<[&str; 2]> for DateRange {
    fn from(value: [&str; 2]) -> Self {
        DateRange::Range(value.iter().map(|s| s.to_string()).collect())
    }
}

impl<'de> Deserialize<'de> for DateRange {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct RangeVisitor;

        impl<'de> Visitor<'de> for RangeVisitor {
            type Value = DateRange;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a date range string or an array of date strings")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(DateRange::Single(v.to_string()))
            }

            fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(DateRange::Single(v))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element::<String>()? {
                    items.push(item);
                }
                Ok(DateRange::Range(items))
            }
        }

        deserializer.deserialize_any(RangeVisitor)
    }
}

/// A time dimension as sent by clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueryTimeDimension {
    pub dimension: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granularity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_range: Option<DateRange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compare_date_range: Option<Vec<DateRange>>,
}

impl QueryTimeDimension {
    pub fn new(dimension: impl Into<String>) -> Self {
        QueryTimeDimension {
            dimension: dimension.into(),
            granularity: None,
            date_range: None,
            compare_date_range: None,
        }
    }
}

/// The `order` of a query: either an object (`{"Cube.member": "asc"}`,
/// insertion order preserved) or an array of `[member, direction]` pairs.
#[derive(Debug, Clone, PartialEq)]
pub enum QueryOrder {
    Map(Vec<(String, OrderDirection)>),
    Pairs(Vec<(String, OrderDirection)>),
}

impl QueryOrder {
    pub fn entries(&self) -> &[(String, OrderDirection)] {
        match self {
            QueryOrder::Map(entries) | QueryOrder::Pairs(entries) => entries,
        }
    }
}

impl<'de> Deserialize<'de> for QueryOrder {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct OrderVisitor;

        impl<'de> Visitor<'de> for OrderVisitor {
            type Value = QueryOrder;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(
                    "an object of member -> direction or an array of [member, direction] pairs",
                )
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut entries = Vec::new();
                while let Some((key, direction)) = map.next_entry::<String, OrderDirection>()? {
                    entries.push((key, direction));
                }
                Ok(QueryOrder::Map(entries))
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut entries = Vec::new();
                while let Some(pair) = seq.next_element::<(String, OrderDirection)>()? {
                    entries.push(pair);
                }
                Ok(QueryOrder::Pairs(entries))
            }
        }

        deserializer.deserialize_any(OrderVisitor)
    }
}

impl Serialize for QueryOrder {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            QueryOrder::Map(entries) => {
                let mut map = serializer.serialize_map(Some(entries.len()))?;
                for (key, direction) in entries {
                    map.serialize_entry(key, direction)?;
                }
                map.end()
            }
            QueryOrder::Pairs(entries) => {
                let mut seq = serializer.serialize_seq(Some(entries.len()))?;
                for pair in entries {
                    seq.serialize_element(pair)?;
                }
                seq.end()
            }
        }
    }
}

/// A subquery join (SQL API).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SubqueryJoin {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<QueryMember>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join_type: Option<JoinType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}

/// A join hint: a join path as a list of cube names.
pub type JoinHint = Vec<String>;

/// A member masked by row level security.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MaskedMember {
    pub member: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<serde_json::Value>,
}

/// An incoming network query (`Query` in `types/query.ts`, validated by the
/// Joi `querySchema` in `query.js`). Unknown fields are rejected.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct Query {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measures: Option<Vec<QueryMember>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<Vec<QueryMember>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filters: Option<Vec<QueryFilter>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_dimensions: Option<Vec<QueryTimeDimension>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<QueryOrder>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segments: Option<Vec<QueryMember>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<bool>,
    /// Set by normalization; accepted on input for re-normalization.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_mode: Option<CacheMode>,
    /// The public cache mode field.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache: Option<CacheMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ungrouped: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResultType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subquery_joins: Option<Vec<SubqueryJoin>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub join_hints: Option<Vec<JoinHint>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub masked_members: Option<Vec<MaskedMember>>,
}

/// A normalized filter condition: `member` (the `dimension` alias resolved),
/// stringified values, relative dates resolved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedFilterLeaf {
    pub member: String,
    pub operator: FilterOperator,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<Option<String>>>,
}

/// A normalized filter: a condition or a boolean `and`/`or` group.
#[derive(Debug, Clone, PartialEq)]
pub enum NormalizedFilter {
    Leaf(NormalizedFilterLeaf),
    Or(Vec<NormalizedFilter>),
    And(Vec<NormalizedFilter>),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawNormalizedFilter {
    #[serde(default)]
    or: Option<Vec<NormalizedFilter>>,
    #[serde(default)]
    and: Option<Vec<NormalizedFilter>>,
    #[serde(default)]
    member: Option<String>,
    #[serde(default)]
    operator: Option<FilterOperator>,
    #[serde(default)]
    values: Option<Vec<Option<String>>>,
}

impl<'de> Deserialize<'de> for NormalizedFilter {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = RawNormalizedFilter::deserialize(deserializer)?;
        match (raw.or, raw.and) {
            (Some(_), Some(_)) => Err(de::Error::custom(
                "contains a conflict between exclusive peers [or, and]",
            )),
            (Some(or), None) => Ok(NormalizedFilter::Or(or)),
            (None, Some(and)) => Ok(NormalizedFilter::And(and)),
            (None, None) => Ok(NormalizedFilter::Leaf(NormalizedFilterLeaf {
                member: raw
                    .member
                    .ok_or_else(|| de::Error::custom("\"member\" is required"))?,
                operator: raw
                    .operator
                    .ok_or_else(|| de::Error::custom("\"operator\" is required"))?,
                values: raw.values,
            })),
        }
    }
}

impl Serialize for NormalizedFilter {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            NormalizedFilter::Leaf(leaf) => leaf.serialize(serializer),
            NormalizedFilter::Or(items) => serialize_group(serializer, "or", items),
            NormalizedFilter::And(items) => serialize_group(serializer, "and", items),
        }
    }
}

/// A normalized time dimension: `dateRange` resolved to an absolute
/// `[start, end]` pair.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedTimeDimension {
    pub dimension: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granularity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date_range: Option<[String; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compare_date_range: Option<Vec<Vec<String>>>,
}

/// A normalized order entry (`{ id, desc }`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderItem {
    pub id: String,
    pub desc: bool,
}

/// A normalized query in the shape handed to the query adapter
/// (`NormalizedQuery` in `types/query.ts` after `remapToQueryAdapterFormat`:
/// `order` as `{ id, desc }` entries and `rowLimit` mirroring `limit`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizedQuery {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measures: Vec<QueryMember>,
    #[serde(default)]
    pub dimensions: Vec<QueryMember>,
    #[serde(default)]
    pub filters: Vec<NormalizedFilter>,
    #[serde(default)]
    pub time_dimensions: Vec<NormalizedTimeDimension>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub segments: Vec<QueryMember>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<Vec<OrderItem>>,
    pub timezone: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<bool>,
    #[serde(default)]
    pub cache_mode: CacheMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ungrouped: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_format: Option<ResultType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subquery_joins: Option<Vec<SubqueryJoin>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join_hints: Option<Vec<JoinHint>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub masked_members: Option<Vec<MaskedMember>>,
    /// Set on the pivot query (and, in Node.js, on the first normalized query
    /// it aliases).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_type: Option<QueryType>,
}

impl From<NormalizedFilter> for QueryFilter {
    fn from(filter: NormalizedFilter) -> Self {
        match filter {
            NormalizedFilter::Leaf(leaf) => QueryFilter::Leaf(FilterLeaf {
                member: Some(leaf.member),
                dimension: None,
                operator: leaf.operator,
                values: leaf.values.map(|values| {
                    values
                        .into_iter()
                        .map(|v| v.map(FilterValue::String).unwrap_or(FilterValue::Null))
                        .collect()
                }),
            }),
            NormalizedFilter::Or(items) => {
                QueryFilter::Or(items.into_iter().map(QueryFilter::from).collect())
            }
            NormalizedFilter::And(items) => {
                QueryFilter::And(items.into_iter().map(QueryFilter::from).collect())
            }
        }
    }
}

impl From<NormalizedTimeDimension> for QueryTimeDimension {
    fn from(td: NormalizedTimeDimension) -> Self {
        QueryTimeDimension {
            dimension: td.dimension,
            granularity: td.granularity,
            date_range: td.date_range.map(|r| DateRange::Range(r.to_vec())),
            compare_date_range: td
                .compare_date_range
                .map(|ranges| ranges.into_iter().map(DateRange::Range).collect()),
        }
    }
}

/// A normalized query can be fed back into normalization (the gateway
/// normalizes again after `queryRewrite` / row level security).
impl From<NormalizedQuery> for Query {
    fn from(q: NormalizedQuery) -> Self {
        Query {
            measures: Some(q.measures),
            dimensions: Some(q.dimensions),
            filters: Some(q.filters.into_iter().map(QueryFilter::from).collect()),
            time_dimensions: Some(
                q.time_dimensions
                    .into_iter()
                    .map(QueryTimeDimension::from)
                    .collect(),
            ),
            order: q.order.map(|order| {
                QueryOrder::Pairs(
                    order
                        .into_iter()
                        .map(|item| {
                            let direction = if item.desc {
                                OrderDirection::Desc
                            } else {
                                OrderDirection::Asc
                            };
                            (item.id, direction)
                        })
                        .collect(),
                )
            }),
            segments: Some(q.segments),
            timezone: Some(q.timezone),
            limit: q.limit,
            offset: q.offset,
            total: q.total,
            cache_mode: Some(q.cache_mode),
            cache: None,
            ungrouped: q.ungrouped,
            response_format: q.response_format,
            subquery_joins: q.subquery_joins,
            join_hints: q.join_hints,
            masked_members: q.masked_members,
        }
    }
}

/// Time dimension of a blending pivot query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PivotTimeDimension {
    pub dimension: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granularity: Option<String>,
}

/// The pivot query of a blending request: the union of the members of all
/// queries over a synthetic `time` dimension.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlendingPivotQuery {
    pub measures: Vec<QueryMember>,
    pub dimensions: Vec<QueryMember>,
    pub time_dimensions: Vec<PivotTimeDimension>,
    pub query_type: QueryType,
}

/// The pivot query returned next to the normalized queries (`getPivotQuery`).
///
/// The variants differ in size, but exactly one value is built per request
/// and it is never stored in a collection, so boxing would only add an
/// indirection to the public API.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum PivotQuery {
    Query(NormalizedQuery),
    Blending(BlendingPivotQuery),
}

/// `InputSqlFunction` of a SQL API member expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InputSqlFunction {
    pub cube_params: Vec<String>,
    pub sql: String,
}

/// The `expr` of a SQL API member expression (aligned with the cubesql side).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub enum InputMemberExpressionExpr {
    SqlFunction {
        cube_params: Vec<String>,
        sql: String,
    },
    PatchMeasure {
        source_measure: String,
        replace_aggregation_type: Option<String>,
        add_filters: Vec<InputSqlFunction>,
    },
}

/// Grouping set of a SQL API member expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InputGroupingSet {
    pub group_type: GroupType,
    pub id: Number,
    #[serde(default)]
    pub sub_id: Option<Number>,
}

/// A SQL API member expression as sent (JSON encoded) inside `measures`,
/// `dimensions`, `segments` or `subqueryJoins[].on`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InputMemberExpression {
    pub cube_name: String,
    pub alias: String,
    pub expr: InputMemberExpressionExpr,
    #[serde(default)]
    pub grouping_set: Option<InputGroupingSet>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deserializes_lenient_shapes() {
        let query: Query = serde_json::from_value(json!({
            "measures": ["Foo.bar"],
            "dimensions": ["Foo.baz", { "expression": ["foo", "return `${foo.x}`"], "cubeName": "foo", "name": "x" }],
            "filters": [
                { "member": "Foo.bar", "operator": "gte", "values": [10.5, 0, true, null, "x"] },
                { "dimension": "Foo.bar", "operator": "set" },
                { "or": [{ "member": "Foo.a", "operator": "equals", "values": ["1"] }, { "and": [] }] }
            ],
            "timeDimensions": [
                { "dimension": "Foo.time", "dateRange": "last 7 days" },
                { "dimension": "Foo.time", "dateRange": ["2020-01-01", "2020-01-02"], "granularity": "day" }
            ],
            "order": { "Foo.bar": "desc", "Foo.baz": "asc" },
            "limit": 10,
            "total": true,
            "responseFormat": "compact",
            "cache": "no-cache"
        }))
        .unwrap();
        assert_eq!(query.measures, Some(vec!["Foo.bar".into()]));
        assert!(matches!(
            query.dimensions.as_ref().unwrap()[1],
            QueryMember::Expression(_)
        ));
        assert_eq!(
            query.order,
            Some(QueryOrder::Map(vec![
                ("Foo.bar".to_string(), OrderDirection::Desc),
                ("Foo.baz".to_string(), OrderDirection::Asc)
            ]))
        );
        assert_eq!(query.limit, Some(10));
        assert_eq!(query.response_format, Some(ResultType::Compact));
        assert_eq!(query.cache, Some(CacheMode::NoCache));

        let pairs: Query = serde_json::from_value(json!({
            "measures": ["Foo.bar"],
            "order": [["Foo.bar", "asc"], ["Foo.foo", "desc"]]
        }))
        .unwrap();
        assert_eq!(
            pairs.order,
            Some(QueryOrder::Pairs(vec![
                ("Foo.bar".to_string(), OrderDirection::Asc),
                ("Foo.foo".to_string(), OrderDirection::Desc)
            ]))
        );
    }

    #[test]
    fn rejects_invalid_shapes() {
        assert!(serde_json::from_value::<Query>(json!({ "measures": "Foo.bar" })).is_err());
        assert!(serde_json::from_value::<Query>(json!({ "foo": 1 })).is_err());
        assert!(serde_json::from_value::<Query>(json!({ "limit": -1 })).is_err());
        assert!(serde_json::from_value::<Query>(json!({ "responseFormat": "arrow" })).is_err());
        assert!(
            serde_json::from_value::<Query>(json!({ "order": [["Foo.bar", "asc", "x"]] })).is_err()
        );
        assert!(serde_json::from_value::<Query>(
            json!({ "filters": [{ "member": "Foo.a", "operator": "nope", "values": [] }] })
        )
        .is_err());
        assert!(
            serde_json::from_value::<Query>(json!({ "filters": [{ "or": [], "and": [] }] }))
                .is_err()
        );
        assert!(
            serde_json::from_value::<Query>(json!({ "filters": [{ "member": "Foo.a" }] })).is_err()
        );
    }

    #[test]
    fn serializes_filters_and_order_in_api_shape() {
        let filter = QueryFilter::Or(vec![QueryFilter::Leaf(FilterLeaf {
            member: Some("Foo.a".into()),
            dimension: None,
            operator: FilterOperator::Equals,
            values: Some(vec!["1".into()]),
        })]);
        assert_eq!(
            serde_json::to_value(&filter).unwrap(),
            json!({ "or": [{ "member": "Foo.a", "operator": "equals", "values": ["1"] }] })
        );
        let order = QueryOrder::Map(vec![
            ("Foo.b".into(), OrderDirection::Desc),
            ("Foo.a".into(), OrderDirection::Asc),
        ]);
        assert_eq!(
            serde_json::to_string(&order).unwrap(),
            r#"{"Foo.b":"desc","Foo.a":"asc"}"#
        );
    }

    #[test]
    fn number_to_string_matches_javascript() {
        let n = |v: serde_json::Value| js_number_to_string(v.as_number().unwrap());
        assert_eq!(n(json!(10.5)), "10.5");
        assert_eq!(n(json!(0)), "0");
        assert_eq!(n(json!(10.0)), "10");
        assert_eq!(n(json!(-3)), "-3");
        assert_eq!(n(json!(1e21)), "1e+21");
        assert_eq!(n(json!(1e-7)), "1e-7");
        assert_eq!(n(json!(0.1)), "0.1");
    }
}
