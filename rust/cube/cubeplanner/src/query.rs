//! The normalized REST query, as the API gateway hands it to the planner.

use crate::error::PlannerError;
use crate::evaluator::Evaluator;
use crate::member_expression::{MemberExpression, QueryMember};
use cubesqlplanner::cube_bridge::base_query_options::{
    BaseQueryOptions, FilterItem, MaskedMemberItem, OrderByItem, TimeDimension,
};
use cubesqlplanner::cube_bridge::join_hints::JoinHintItem;
use cubesqlplanner::cube_bridge::options_member::OptionsMember;
use cubesqlplanner::cube_bridge::subquery_join::SubqueryJoin as SubqueryJoinTrait;
use cubesqlplanner::rust_model::{MockBaseQueryOptions, MockSubqueryJoin};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::rc::Rc;

/// A query in Cube's normalized REST shape.
///
/// Deserializes straight from the JSON body of `/v1/load` after the gateway has
/// normalized it: member names are fully qualified, `timeDimensions` carry a
/// granularity and/or a date range, `filters` may nest boolean groups.
///
/// `measures`, `dimensions` and `segments` also accept a *member expression* in
/// place of a name — the shape the SQL API pushes down. See
/// [`crate::member_expression`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PlannerQuery {
    pub measures: Vec<QueryMember>,
    pub dimensions: Vec<QueryMember>,
    pub segments: Vec<QueryMember>,
    pub time_dimensions: Vec<TimeDimension>,
    pub filters: Vec<FilterItem>,
    #[serde(deserialize_with = "deserialize_order")]
    pub order: Vec<OrderByItem>,
    /// `limit` and `rowLimit` are the same thing; both spellings are accepted.
    #[serde(alias = "rowLimit", deserialize_with = "deserialize_count")]
    pub limit: Option<String>,
    #[serde(deserialize_with = "deserialize_count")]
    pub offset: Option<String>,
    pub ungrouped: Option<bool>,
    pub timezone: Option<String>,
    /// Alias overrides keyed by member name, as the SQL API sends them.
    pub member_to_alias: Option<HashMap<String, String>>,
    /// Render the grand total instead of the rows.
    pub total_query: Option<bool>,

    /// Cubes the join must pass through, over and above the ones the query's
    /// members imply. The SQL API sends these when it has pushed a join down.
    pub join_hints: Vec<JoinHint>,
    /// Joins against an opaque, already-rendered sub-select — the SQL API's
    /// `subqueryJoins`.
    pub subquery_joins: Vec<SubqueryJoin>,
    /// Members whose value is replaced by their `mask` unless the request's
    /// filter says otherwise.
    pub masked_members: Vec<MaskedMemberItem>,

    /// Plan only this pre-aggregation, by id (`cube.preAggregation`).
    pub pre_aggregation_id: Option<String>,
    /// This query *builds* a pre-aggregation, so it is not itself served from
    /// one.
    pub pre_aggregation_query: Option<bool>,
    /// Ignore external (CubeStore) pre-aggregations when matching.
    pub disable_external_pre_aggregations: Option<bool>,
    /// Whether the external store can run a multi-stage query, which decides
    /// if a multi-stage query may be served from a rollup at all.
    pub cubestore_support_multistage: Option<bool>,

    /// Annotate the generated SQL with the members each expression came from.
    /// Overrides [`crate::PlanOptions::export_annotated_sql`] when set.
    pub export_annotated_sql: Option<bool>,
    /// Convert a raw (non-granular) time dimension into the query timezone.
    /// Overrides [`crate::PlanOptions::convert_tz_for_raw_time_dimension`].
    pub convert_tz_for_raw_time_dimension: Option<bool>,
}

/// The two request-level switches that reach both the planning state and the
/// query options. Each is set by the query, falling back to
/// [`crate::PlanOptions`].
#[derive(Debug, Clone, Copy)]
pub(crate) struct RequestFlags {
    pub export_annotated_sql: bool,
    pub convert_tz_for_raw_time_dimension: bool,
}

/// One `joinHints` entry: a cube, or a join path through several cubes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JoinHint {
    Single(String),
    Path(Vec<String>),
}

impl JoinHint {
    fn to_hint_item(&self) -> JoinHintItem {
        match self {
            Self::Single(name) => JoinHintItem::Single(name.clone()),
            Self::Path(path) => JoinHintItem::Vector(path.clone()),
        }
    }
}

/// A join against a sub-select the caller already rendered. `sql` is a whole
/// SELECT and `on` is the join condition, written as a member expression.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubqueryJoin {
    pub sql: String,
    pub alias: String,
    #[serde(default)]
    pub join_type: Option<String>,
    pub on: MemberExpression,
}

impl PlannerQuery {
    /// Parses a normalized REST query from JSON.
    pub fn from_json(json: &str) -> Result<Self, PlannerError> {
        serde_json::from_str(json)
            .map_err(|e| PlannerError::query(format!("Failed to parse query: {e}")))
    }

    /// Parses a normalized REST query from an already-decoded JSON value.
    pub fn from_value(value: Value) -> Result<Self, PlannerError> {
        serde_json::from_value(value)
            .map_err(|e| PlannerError::query(format!("Failed to parse query: {e}")))
    }

    /// Whether the SQL carries the member each expression came from.
    pub(crate) fn export_annotated_sql(&self, default: bool) -> bool {
        self.export_annotated_sql.unwrap_or(default)
    }

    /// Whether a raw time dimension is converted into the query timezone.
    pub(crate) fn convert_tz_for_raw_time_dimension(&self, default: bool) -> bool {
        self.convert_tz_for_raw_time_dimension.unwrap_or(default)
    }

    /// Whether the external store may run a multi-stage query.
    /// `BaseQuery.try_new` reads the same flag and defaults it to `false`.
    pub(crate) fn cubestore_support_multistage(&self) -> bool {
        self.cubestore_support_multistage.unwrap_or(false)
    }

    pub(crate) fn masked_members(&self) -> Option<Vec<MaskedMemberItem>> {
        non_empty(self.masked_members.clone())
    }

    fn validate(&self) -> Result<(), PlannerError> {
        if self.measures.is_empty() && self.dimensions.is_empty() && self.time_dimensions.is_empty()
        {
            return Err(PlannerError::query(
                "A query needs at least one measure, dimension or time dimension".to_string(),
            ));
        }
        Ok(())
    }

    /// Builds the planner's `BaseQueryOptions` for this query.
    pub(crate) fn to_options(
        &self,
        evaluator: &Evaluator,
        timezone: &str,
        flags: RequestFlags,
    ) -> Result<Rc<dyn BaseQueryOptions>, PlannerError> {
        self.validate()?;

        Ok(Rc::new(
            MockBaseQueryOptions::builder()
                .cube_evaluator(evaluator.cube_evaluator())
                .base_tools(evaluator.base_tools())
                .join_graph(evaluator.join_graph())
                .security_context(evaluator.security_context())
                .measures(members(&self.measures)?)
                .dimensions(members(&self.dimensions)?)
                .segments(members(&self.segments)?)
                .time_dimensions(non_empty(self.time_dimensions.clone()))
                .filters(non_empty(self.filters.clone()))
                .order(non_empty(self.order.clone()))
                .limit(self.limit.clone())
                .row_limit(self.limit.clone())
                .offset(self.offset.clone())
                .ungrouped(self.ungrouped)
                .total_query(self.total_query)
                .member_to_alias(self.member_to_alias.clone())
                .timezone(Some(timezone.to_string()))
                .join_hints(non_empty(
                    self.join_hints.iter().map(JoinHint::to_hint_item).collect(),
                ))
                .subquery_joins(self.subquery_joins()?)
                .masked_members(self.masked_members())
                .pre_aggregation_id(self.pre_aggregation_id.clone())
                .pre_aggregation_query(self.pre_aggregation_query)
                .disable_external_pre_aggregations(
                    self.disable_external_pre_aggregations.unwrap_or(false),
                )
                .cubestore_support_multistage(self.cubestore_support_multistage)
                .export_annotated_sql(flags.export_annotated_sql)
                .convert_tz_for_raw_time_dimension(Some(flags.convert_tz_for_raw_time_dimension))
                .build(),
        ))
    }

    fn subquery_joins(&self) -> Result<Option<Vec<Rc<dyn SubqueryJoinTrait>>>, PlannerError> {
        if self.subquery_joins.is_empty() {
            return Ok(None);
        }
        let joins = self
            .subquery_joins
            .iter()
            .map(|join| -> Result<Rc<dyn SubqueryJoinTrait>, PlannerError> {
                let on = match join.on.to_options_member()? {
                    OptionsMember::MemberExpression(expression) => expression,
                    OptionsMember::MemberName(name) => {
                        return Err(PlannerError::query(format!(
                            "A `subqueryJoins` entry's `on` must be a member expression, got the \
                             member name `{name}`"
                        )))
                    }
                };
                Ok(Rc::new(
                    MockSubqueryJoin::builder()
                        .sql(join.sql.clone())
                        .alias(join.alias.clone())
                        .join_type(join.join_type.clone())
                        .on(on)
                        .build(),
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(joins))
    }
}

fn members(members: &[QueryMember]) -> Result<Option<Vec<OptionsMember>>, PlannerError> {
    Ok(non_empty(
        members
            .iter()
            .map(QueryMember::to_options_member)
            .collect::<Result<Vec<_>, _>>()?,
    ))
}

fn non_empty<T>(items: Vec<T>) -> Option<Vec<T>> {
    if items.is_empty() {
        None
    } else {
        Some(items)
    }
}

/// `limit` / `offset` reach the planner as strings, but a REST query sends
/// numbers.
fn deserialize_count<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(match value {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(other) => {
            return Err(serde::de::Error::custom(format!(
                "expected a number or a string, got {other}"
            )))
        }
    })
}

/// Accepts both spellings of `order`: the normalized `[{id, desc}]` and the
/// `[["member", "asc"]]` form the REST API also takes.
fn deserialize_order<'de, D>(deserializer: D) -> Result<Vec<OrderByItem>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OrderEntry {
        Item {
            id: String,
            #[serde(default)]
            desc: Option<bool>,
        },
        Pair(String, String),
    }

    let entries = Vec::<OrderEntry>::deserialize(deserializer)?;
    entries
        .into_iter()
        .map(|entry| match entry {
            OrderEntry::Item { id, desc } => Ok(OrderByItem { id, desc }),
            OrderEntry::Pair(id, direction) => match direction.to_ascii_lowercase().as_str() {
                "asc" => Ok(OrderByItem {
                    id,
                    desc: Some(false),
                }),
                "desc" => Ok(OrderByItem {
                    id,
                    desc: Some(true),
                }),
                other => Err(serde::de::Error::custom(format!(
                    "order direction must be `asc` or `desc`, got `{other}`"
                ))),
            },
        })
        .collect()
}
