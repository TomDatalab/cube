//! Port of the request-side query handling of `gateway.ts`:
//! `parseQueryParam`, `compareDateRangeTransformer`, `getNormalizedQueries`
//! and `getPivotQuery`.

use serde_json::Value;

use crate::config::QueryConfig;
use crate::error::QueryError;
use crate::normalize::{get_query_granularity, normalize_query};
use crate::types::{
    BlendingPivotQuery, CacheMode, DateRange, NormalizedQuery, PivotQuery, PivotTimeDimension,
    Query, QueryMember, QueryTimeDimension, QueryType,
};

/// Port of `ApiGateway.parseQueryParam`: the `query` of a GET request is a
/// JSON document in the query string, the one of a POST request is already
/// parsed. Either may hold a single query or an array of them.
pub fn parse_query_param(value: &Value) -> Result<Vec<Query>, QueryError> {
    let parsed: Value = match value {
        Value::Null => return Err(QueryError::user("Query param is required")),
        Value::String(raw) if raw.is_empty() || raw == "undefined" => {
            return Err(QueryError::user("Query param is required"))
        }
        Value::String(raw) => serde_json::from_str(raw).map_err(|e| {
            QueryError::user(format!(
                "Unable to decode query param as JSON, error: {}",
                e
            ))
        })?,
        other => other.clone(),
    };

    queries_from_value(parsed)
}

fn queries_from_value(value: Value) -> Result<Vec<Query>, QueryError> {
    match value {
        Value::Array(items) => items
            .into_iter()
            .map(|item| {
                serde_json::from_value(item)
                    .map_err(|e| QueryError::user(format!("Invalid query format: {}", e)))
            })
            .collect(),
        other => serde_json::from_value(other)
            .map(|q| vec![q])
            .map_err(|e| QueryError::user(format!("Invalid query format: {}", e))),
    }
}

/// Port of `ApiGateway.compareDateRangeTransformer`: a `compareDateRange` on
/// one time dimension fans the query out into one query per range.
pub fn compare_date_range_transformer(query: &Query) -> Result<Vec<Query>, QueryError> {
    let time_dimensions = query.time_dimensions.clone().unwrap_or_default();

    let mut found: Option<(usize, Vec<DateRange>)> = None;
    for (index, td) in time_dimensions.iter().enumerate() {
        if let Some(ranges) = td.compare_date_range.clone() {
            if found.is_some() {
                return Err(QueryError::user(
                    "compareDateRange can only exist for one timeDimension",
                ));
            }
            found = Some((index, ranges));
        }
    }

    let Some((target_index, ranges)) = found else {
        return Ok(vec![query.clone()]);
    };

    Ok(ranges
        .into_iter()
        .map(|range| {
            let mut expanded = query.clone();
            expanded.time_dimensions = Some(
                time_dimensions
                    .iter()
                    .enumerate()
                    .map(|(index, td)| {
                        if index == target_index {
                            QueryTimeDimension {
                                dimension: td.dimension.clone(),
                                granularity: td.granularity.clone(),
                                date_range: Some(range.clone()),
                                compare_date_range: None,
                            }
                        } else {
                            td.clone()
                        }
                    })
                    .collect(),
            );
            expanded
        })
        .collect())
}

/// Port of `ApiGateway.getNormalizedQueries`: decides the query type and
/// normalizes every query of the request.
pub fn get_normalized_queries(
    input: &Value,
    persistent: bool,
    cache_mode: Option<CacheMode>,
    config: &QueryConfig,
) -> Result<(QueryType, Vec<NormalizedQuery>), QueryError> {
    let parsed = parse_query_param(input)?;
    let was_array = matches!(input, Value::Array(_))
        || matches!(input, Value::String(raw) if raw.trim_start().starts_with('['));

    let (query_type, queries) = if was_array {
        (QueryType::BlendingQuery, parsed)
    } else {
        let single = parsed
            .into_iter()
            .next()
            .ok_or_else(|| QueryError::user("Query param is required"))?;
        let expanded = compare_date_range_transformer(&single)?;
        if expanded.len() > 1 {
            (QueryType::CompareDateRangeQuery, expanded)
        } else {
            (QueryType::RegularQuery, expanded)
        }
    };

    let normalized = queries
        .iter()
        .map(|query| normalize_query(query, persistent, cache_mode, config))
        .collect::<Result<Vec<_>, _>>()?;

    Ok((query_type, normalized))
}

/// Port of `getPivotQuery`.
pub fn get_pivot_query(
    query_type: QueryType,
    queries: &[NormalizedQuery],
) -> Result<PivotQuery, QueryError> {
    let first = queries
        .first()
        .ok_or_else(|| QueryError::internal("getPivotQuery called without queries"))?;

    match query_type {
        QueryType::BlendingQuery => {
            let granularity = get_query_granularity(queries).into_iter().next();
            Ok(PivotQuery::Blending(BlendingPivotQuery {
                measures: unique_members(queries.iter().flat_map(|q| q.measures.iter().cloned())),
                dimensions: unique_members(
                    queries.iter().flat_map(|q| q.dimensions.iter().cloned()),
                ),
                time_dimensions: vec![PivotTimeDimension {
                    dimension: "time".to_string(),
                    granularity,
                }],
                query_type,
            }))
        }
        QueryType::CompareDateRangeQuery => {
            let mut pivot = first.clone();
            let mut dimensions = vec![QueryMember::Name("compareDateRange".to_string())];
            dimensions.extend(pivot.dimensions.iter().cloned());
            pivot.dimensions = dimensions;
            pivot.query_type = Some(query_type);
            Ok(PivotQuery::Query(pivot))
        }
        QueryType::RegularQuery => {
            let mut pivot = first.clone();
            pivot.query_type = Some(query_type);
            Ok(PivotQuery::Query(pivot))
        }
    }
}

fn unique_members(members: impl Iterator<Item = QueryMember>) -> Vec<QueryMember> {
    let mut seen: Vec<QueryMember> = Vec::new();
    for member in members {
        if !seen.contains(&member) {
            seen.push(member);
        }
    }
    seen
}
