//! Port of `normalizeQuery` and its helpers from
//! `packages/cubejs-api-gateway/src/query.js`.

use std::sync::LazyLock;

use regex::Regex;

use crate::config::QueryConfig;
use crate::date_parser::date_parser;
use crate::error::QueryError;
use crate::moment;
use crate::timezone::canonical_timezone;
use crate::types::{
    CacheMode, DateRange, FilterLeaf, FilterOperator, NormalizedFilter, NormalizedFilterLeaf,
    NormalizedQuery, NormalizedTimeDimension, OrderItem, Query, QueryFilter, QueryMember,
    QueryTimeDimension,
};

/// `^\d\d\d\d-\d\d-\d\d$`
static DATE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d\d\d\d-\d\d-\d\d$").expect("valid regex"));

/// `^\d{4}-\d{2}-\d{2}([T ]\d{2}:\d{2}(:\d{2}(\.\d{1,6})?)?(Z|[+-]\d{2}(:?\d{2})?)?)?$`
static ABSOLUTE_DATE_TIME_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^\d{4}-\d{2}-\d{2}([T ]\d{2}:\d{2}(:\d{2}(\.\d{1,6})?)?(Z|[+-]\d{2}(:?\d{2})?)?)?$",
    )
    .expect("valid regex")
});

const DATE_RANGE_OPERATORS: [FilterOperator; 2] =
    [FilterOperator::InDateRange, FilterOperator::NotInDateRange];
/// Before → `< start`, AfterOrOn → `>= start`.
const START_DATE_OPERATORS: [FilterOperator; 2] =
    [FilterOperator::BeforeDate, FilterOperator::AfterOrOnDate];
/// BeforeOrOn → `<= end`, After → `> end`.
const END_DATE_OPERATORS: [FilterOperator; 2] =
    [FilterOperator::BeforeOrOnDate, FilterOperator::AfterDate];

/// Operators that accept an empty `values` list.
const VALUELESS_OPERATORS: [FilterOperator; 3] = [
    FilterOperator::Set,
    FilterOperator::NotSet,
    FilterOperator::MeasureFilter,
];

/// `moment.utc(d).format(...)` of `resolveDateRange`: a bare date becomes the
/// start or the end of that day, a timestamp keeps its time component.
fn format_bound(value: &str, is_start: bool) -> Result<String, QueryError> {
    let naive = moment::parse_utc(value)
        .ok_or_else(|| QueryError::user(format!("Invalid date: {}", value)))?;

    if DATE_REGEX.is_match(value) {
        let day = naive.date();
        Ok(format!(
            "{}T{}",
            day.format("%Y-%m-%d"),
            if is_start {
                "00:00:00.000"
            } else {
                "23:59:59.999"
            }
        ))
    } else {
        Ok(moment::format_naive(&naive))
    }
}

/// Port of `resolveDateRange`: a relative string, a single absolute date or a
/// two element array resolved to `[start, end]`.
pub fn resolve_date_range(input: &DateRange, timezone: &str) -> Result<[String; 2], QueryError> {
    let bounds: [String; 2] = match input {
        DateRange::Single(value) => date_parser(value, timezone)?,
        DateRange::Range(values) => match values.as_slice() {
            [single] => [single.clone(), single.clone()],
            [start, end] => [start.clone(), end.clone()],
            other => {
                return Err(QueryError::user(format!(
                    "dateRange must have one or two elements, got {}",
                    other.len()
                )))
            }
        },
    };

    Ok([
        format_bound(&bounds[0], true)?,
        format_bound(&bounds[1], false)?,
    ])
}

/// Port of `normalizeDateFilterValues`: resolves relative date strings inside
/// a filter leaf so date filters work anywhere, including inside groups.
fn normalize_date_filter_values(
    leaf: NormalizedFilterLeaf,
    timezone: &str,
) -> Result<NormalizedFilterLeaf, QueryError> {
    let Some(values) = leaf.values.as_ref() else {
        return Ok(leaf);
    };

    let is_range_operator =
        DATE_RANGE_OPERATORS.contains(&leaf.operator) || leaf.operator == FilterOperator::OnTheDate;

    // Fail fast: a range operator with several values, one of them relative,
    // would otherwise fail deep in query execution with an opaque message.
    if is_range_operator
        && values.len() > 1
        && values.iter().any(|v| {
            v.as_deref()
                .is_some_and(|v| !ABSOLUTE_DATE_TIME_REGEX.is_match(v))
        })
    {
        return Err(QueryError::user(format!(
            "Relative-date strings are only supported when `values` has a single element for operator `{}`. \
             Pass an absolute two-element [start, end] pair, or a single relative string like [\"last 2 weeks\"]. Got: {}",
            leaf.operator.as_str(),
            serde_json::to_string(values).unwrap_or_else(|_| "[]".to_string())
        )));
    }

    if values.len() != 1 {
        return Ok(leaf);
    }
    let Some(value) = values[0].clone() else {
        return Ok(leaf);
    };

    if is_range_operator {
        // `onTheDate` resolves to a two sided range, like `inDateRange`.
        let range = resolve_date_range(&DateRange::Single(value), timezone)?;
        return Ok(NormalizedFilterLeaf {
            values: Some(range.into_iter().map(Some).collect()),
            ..leaf
        });
    }

    if ABSOLUTE_DATE_TIME_REGEX.is_match(&value) {
        return Ok(leaf);
    }

    if START_DATE_OPERATORS.contains(&leaf.operator) {
        let [start, _] = resolve_date_range(&DateRange::Single(value), timezone)?;
        return Ok(NormalizedFilterLeaf {
            values: Some(vec![Some(start)]),
            ..leaf
        });
    }

    if END_DATE_OPERATORS.contains(&leaf.operator) {
        let [_, end] = resolve_date_range(&DateRange::Single(value), timezone)?;
        return Ok(NormalizedFilterLeaf {
            values: Some(vec![Some(end)]),
            ..leaf
        });
    }

    Ok(leaf)
}

fn filter_json(leaf: &FilterLeaf) -> String {
    serde_json::to_string(leaf).unwrap_or_else(|_| "{}".to_string())
}

/// Port of `normalizeQueryFilters`.
pub fn normalize_query_filters(
    filters: &[QueryFilter],
    timezone: &str,
) -> Result<Vec<NormalizedFilter>, QueryError> {
    filters
        .iter()
        .map(|filter| match filter {
            QueryFilter::Or(items) => Ok(NormalizedFilter::Or(normalize_query_filters(
                items, timezone,
            )?)),
            QueryFilter::And(items) => Ok(NormalizedFilter::And(normalize_query_filters(
                items, timezone,
            )?)),
            QueryFilter::Leaf(leaf) => {
                let has_values = leaf.values.as_ref().is_some_and(|v| !v.is_empty());
                if !has_values && !VALUELESS_OPERATORS.contains(&leaf.operator) {
                    return Err(QueryError::user(format!(
                        "Values required for filter: {}",
                        filter_json(leaf)
                    )));
                }

                // JS: `v != null ? v.toString() : v`
                let values = leaf
                    .values
                    .as_ref()
                    .map(|values| values.iter().map(|v| v.to_js_string()).collect());

                // `dimension` is the legacy alias of `member`.
                let member = leaf
                    .member
                    .clone()
                    .or_else(|| leaf.dimension.clone())
                    .ok_or_else(|| {
                        QueryError::user(format!(
                            "Member required for filter: {}",
                            filter_json(leaf)
                        ))
                    })?;

                Ok(NormalizedFilter::Leaf(normalize_date_filter_values(
                    NormalizedFilterLeaf {
                        member,
                        operator: leaf.operator,
                        values,
                    },
                    timezone,
                )?))
            }
        })
        .collect()
}

/// Port of `normalizeQueryOrder` + `remapQueryOrder`: both object and array
/// input become `{ id, desc }` entries.
pub fn normalize_query_order(order: &crate::types::QueryOrder) -> Vec<OrderItem> {
    order
        .entries()
        .iter()
        .map(|(id, direction)| OrderItem {
            id: id.clone(),
            desc: *direction == crate::types::OrderDirection::Desc,
        })
        .collect()
}

/// A dimension written as `cube.dimension.granularity`.
fn split_dimension_with_granularity(member: &QueryMember) -> Option<(String, String)> {
    let name = member.as_name()?;
    let parts: Vec<&str> = name.split('.').collect();
    if parts.len() == 3 {
        Some((format!("{}.{}", parts[0], parts[1]), parts[2].to_string()))
    } else {
        None
    }
}

/// Port of `normalizeQueryCacheMode`.
fn normalize_cache_mode(query: &Query, cache_mode: Option<CacheMode>) -> CacheMode {
    cache_mode
        .or(query.cache)
        .or(query.cache_mode)
        .unwrap_or(CacheMode::StaleIfSlow)
}

/// Port of `normalizeQuery(query, persistent, cacheMode)` followed by
/// `remapToQueryAdapterFormat`.
pub fn normalize_query(
    query: &Query,
    persistent: bool,
    cache_mode: Option<CacheMode>,
    config: &QueryConfig,
) -> Result<NormalizedQuery, QueryError> {
    let cache_mode = normalize_cache_mode(query, cache_mode);

    let requested_timezone = query
        .timezone
        .clone()
        .filter(|tz| !tz.is_empty())
        .unwrap_or_else(|| config.default_timezone.clone());
    let timezone = canonical_timezone(&requested_timezone).ok_or_else(|| {
        QueryError::user(format!(
            "Invalid query format: \"timezone\" must be a valid timezone, got {}",
            requested_timezone
        ))
    })?;

    let measures = query.measures.clone().unwrap_or_default();
    let dimensions = query.dimensions.clone().unwrap_or_default();
    let time_dimensions = query.time_dimensions.clone().unwrap_or_default();

    let has_granularity = time_dimensions.iter().any(|td| td.granularity.is_some());
    if measures.is_empty() && dimensions.is_empty() && !has_granularity {
        return Err(QueryError::user(
            "Query should contain either measures, dimensions or timeDimensions with granularities in order to be valid",
        ));
    }

    // `cube.dimension.granularity` entries move to timeDimensions.
    let mut regular_to_time_dimension: Vec<NormalizedTimeDimension> = Vec::new();
    let mut plain_dimensions: Vec<QueryMember> = Vec::new();
    for dimension in dimensions {
        match split_dimension_with_granularity(&dimension) {
            Some((name, granularity)) => regular_to_time_dimension.push(NormalizedTimeDimension {
                dimension: name,
                granularity: Some(granularity),
                date_range: None,
                compare_date_range: None,
            }),
            None => plain_dimensions.push(dimension),
        }
    }

    let limit = if persistent {
        query.limit
    } else {
        match query.limit {
            Some(limit) if limit > config.db_query_limit => {
                // A plain `Error` in Node.js, i.e. HTTP 500.
                return Err(QueryError::internal("The query limit has been exceeded."));
            }
            Some(limit) => Some(limit),
            None => Some(config.effective_default_limit()),
        }
    };

    let mut normalized_time_dimensions = time_dimensions
        .iter()
        .map(|td| normalize_time_dimension(td, &timezone))
        .collect::<Result<Vec<_>, _>>()?;
    normalized_time_dimensions.extend(regular_to_time_dimension);

    Ok(NormalizedQuery {
        measures,
        dimensions: plain_dimensions,
        filters: normalize_query_filters(query.filters.as_deref().unwrap_or_default(), &timezone)?,
        time_dimensions: normalized_time_dimensions,
        segments: query.segments.clone().unwrap_or_default(),
        order: query.order.as_ref().map(normalize_query_order),
        timezone,
        limit,
        // `remapToQueryAdapterFormat`: `rowLimit` mirrors `limit`.
        row_limit: limit,
        offset: query.offset,
        total: query.total,
        cache_mode,
        ungrouped: query.ungrouped,
        response_format: query.response_format,
        subquery_joins: query.subquery_joins.clone(),
        join_hints: query.join_hints.clone(),
        masked_members: query.masked_members.clone(),
        query_type: None,
    })
}

fn normalize_time_dimension(
    td: &QueryTimeDimension,
    timezone: &str,
) -> Result<NormalizedTimeDimension, QueryError> {
    let date_range = td
        .date_range
        .as_ref()
        .map(|range| resolve_date_range(range, timezone))
        .transpose()?;

    let compare_date_range = td
        .compare_date_range
        .as_ref()
        .map(|ranges| {
            ranges
                .iter()
                .map(|range| match range {
                    // Only a relative string is resolved here; an explicit
                    // pair is passed through as-is by the Node.js code.
                    DateRange::Single(value) => {
                        date_parser(value, timezone).map(|bounds| bounds.to_vec())
                    }
                    DateRange::Range(values) => Ok(values.clone()),
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;

    Ok(NormalizedTimeDimension {
        dimension: td.dimension.clone(),
        granularity: td.granularity.clone(),
        date_range,
        compare_date_range,
    })
}

/// Port of `getQueryGranularity`: the distinct granularities of the first time
/// dimension of each query.
pub fn get_query_granularity(queries: &[NormalizedQuery]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for query in queries {
        if let Some(granularity) = query
            .time_dimensions
            .first()
            .and_then(|td| td.granularity.clone())
        {
            if !seen.contains(&granularity) {
                seen.push(granularity);
            }
        }
    }
    seen
}
