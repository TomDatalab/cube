//! The public planning entry point.

use crate::error::PlannerError;
use crate::evaluator::Evaluator;
use crate::model::Model;
use crate::options::PlanOptions;
use crate::query::{PlannerQuery, RequestFlags};
use chrono_tz::Tz;
use cubesqlplanner::cube_bridge::base_query_options::FilterValue;
use cubesqlplanner::planner::{
    MemberSymbol, QueryProperties, QueryPropertiesCompiler, State, TopLevelPlanner,
};
use std::collections::HashMap;

/// A SQL parameter bound to a `$n` / `?` placeholder in [`PlannedSql::sql`].
pub type QueryParam = FilterValue;

/// The SQL the planner produced, with its parameters in placeholder order.
#[derive(Debug, Clone)]
pub struct PlannedSql {
    pub sql: String,
    pub params: Vec<QueryParam>,
    /// The result set's column names mapped back to the members they answer
    /// for — `orders__status` to `orders.status`, and a granular time
    /// dimension's `orders__created_at_month` to `orders.created_at.month`.
    ///
    /// The REST `/v1/load` response is keyed by member, while the SQL is keyed
    /// by alias, so this is what turns one into the other. It is the Rust
    /// counterpart of `BaseQuery.aliasNameToMember`
    /// (`adapter/BaseQuery.js:606-615`), built from the planner's own aliases
    /// rather than from the naming convention.
    ///
    /// A member expression names no model member, so it maps to the planner's
    /// own identity for it, `expr:<cube>.<expressionName>` — the JS map holds
    /// the expression object itself in that slot, for the same reason.
    pub alias_name_to_member: HashMap<String, String>,
}

impl PlannedSql {
    /// The parameters as plain strings, the way a driver binds them. A `NULL`
    /// parameter yields `None`.
    pub fn param_strings(&self) -> Vec<Option<String>> {
        self.params.iter().map(|p| p.to_param_string()).collect()
    }
}

/// Plans one query against one model and renders it as SQL.
///
/// This is the whole Tesseract pipeline with no JavaScript in it: the model's
/// member SQL is compiled by the Rust member-sql parser, join paths come from
/// the Rust join graph, and the SQL is rendered through the dialect's Jinja
/// templates.
pub fn plan(
    model: &Model,
    query: &PlannerQuery,
    options: &PlanOptions,
) -> Result<PlannedSql, PlannerError> {
    let timezone = resolve_timezone(query, options)?;
    let evaluator = Evaluator::new(model, options, &timezone.to_string())?;

    // Both flags are request-level in JS: they live on the same options object
    // the query does, so a query that carries one overrides the caller's
    // default for this request.
    let flags = RequestFlags {
        export_annotated_sql: query.export_annotated_sql(options.export_annotated_sql),
        convert_tz_for_raw_time_dimension: query
            .convert_tz_for_raw_time_dimension(options.convert_tz_for_raw_time_dimension),
    };

    let state = State::try_new(
        evaluator.cube_evaluator(),
        evaluator.security_context(),
        evaluator.base_tools(),
        evaluator.join_graph(),
        Some(timezone.to_string()),
        flags.export_annotated_sql,
        flags.convert_tz_for_raw_time_dimension,
        query.masked_members(),
        query.member_to_alias.clone(),
    )?;

    let query_options = query.to_options(&evaluator, &timezone.to_string(), flags)?;

    let properties = QueryPropertiesCompiler::new(state.clone()).build(query_options)?;
    let (raw_sql, pre_aggregation_usages) = TopLevelPlanner::new(
        properties.clone(),
        state.clone(),
        query.cubestore_support_multistage(),
    )
    .plan()?;

    // A query fully covered by external pre-aggregations is read from the
    // external store, so it renders in that store's dialect.
    let is_external = !pre_aggregation_usages.is_empty()
        && pre_aggregation_usages
            .iter()
            .all(|usage| usage.pre_aggregation.external());

    let templates = state.plan_sql_templates(is_external)?;
    let (sql, params) = state.build_sql_and_params(&raw_sql, &templates)?;
    if !is_external {
        options.dialect.check_rendered_sql(&sql)?;
    }

    Ok(PlannedSql {
        sql,
        params,
        alias_name_to_member: alias_name_to_member(&properties),
    })
}

/// `BaseQuery.aliasNameToMember`: every selected member's alias mapped to the
/// member it answers for. A time dimension is keyed `<dimension>.<granularity>`
/// and one without a granularity is left out, exactly as the JS does — a raw
/// time dimension reaches the response as the plain dimension it is.
fn alias_name_to_member(properties: &QueryProperties) -> HashMap<String, String> {
    let mut aliases = HashMap::new();

    for symbol in properties.measures().iter().chain(properties.dimensions()) {
        aliases.insert(symbol.alias(), symbol.full_name());
    }

    for symbol in properties.time_dimensions() {
        let MemberSymbol::TimeDimension(time_dimension) = symbol.as_ref() else {
            continue;
        };
        let Some(granularity) = time_dimension.granularity() else {
            continue;
        };
        aliases.insert(
            symbol.alias(),
            format!(
                "{}.{}",
                time_dimension.base_symbol().full_name(),
                granularity
            ),
        );
    }

    aliases
}

fn resolve_timezone(query: &PlannerQuery, options: &PlanOptions) -> Result<Tz, PlannerError> {
    let name = query
        .timezone
        .clone()
        .or_else(|| options.timezone.clone())
        .unwrap_or_else(|| "UTC".to_string());

    name.parse::<Tz>()
        .map_err(|_| PlannerError::query(format!("Unknown timezone `{name}`")))
}
