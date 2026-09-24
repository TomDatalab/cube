//! Ported from `packages/cubejs-api-gateway/test/normalize-query.test.ts` and
//! `test/normalize-query-filters-dates.test.js`.

use cubequery::types::*;
use cubequery::{
    compare_date_range_transformer, get_normalized_queries, get_pivot_query, normalize_query,
    normalize_query_filters, parse_query_param, QueryConfig, QueryError,
};
use serde_json::{json, Value};

fn config() -> QueryConfig {
    QueryConfig::default()
}

fn query_from(value: Value) -> Query {
    serde_json::from_value(value).expect("valid query")
}

fn normalize(value: Value) -> Result<NormalizedQuery, QueryError> {
    normalize_query(&query_from(value), false, None, &config())
}

fn base() -> Value {
    json!({ "measures": ["Orders.count"], "timezone": "UTC" })
}

fn leaf_values(filter: &NormalizedFilter) -> Vec<Option<String>> {
    match filter {
        NormalizedFilter::Leaf(leaf) => leaf.values.clone().unwrap_or_default(),
        other => panic!("expected a leaf, got {:?}", other),
    }
}

fn filters_of(value: Value) -> Vec<NormalizedFilter> {
    let filters: Vec<QueryFilter> = serde_json::from_value(value).expect("valid filters");
    normalize_query_filters(&filters, "UTC").expect("normalized")
}

// ---------------------------------------------------------------- filters

#[test]
fn in_date_range_with_relative_string_resolves_to_absolute_pair() {
    let values = leaf_values(
        &filters_of(json!([
            { "member": "Orders.createdAt", "operator": "inDateRange", "values": ["last 2 weeks"] }
        ]))[0],
    );

    assert_eq!(values.len(), 2);
    let start = values[0].clone().unwrap();
    let end = values[1].clone().unwrap();
    assert!(start.ends_with("T00:00:00.000"), "{start}");
    assert!(end.ends_with("T23:59:59.999"), "{end}");
}

#[test]
fn not_in_date_range_resolves_like_in_date_range() {
    let values = leaf_values(
        &filters_of(json!([
            { "member": "Orders.createdAt", "operator": "notInDateRange", "values": ["last 2 weeks"] }
        ]))[0],
    );
    assert_eq!(values.len(), 2);
}

#[test]
fn absolute_two_element_range_passes_through_unchanged() {
    let values = leaf_values(
        &filters_of(json!([
            { "member": "Orders.createdAt", "operator": "inDateRange", "values": ["2026-01-01", "2026-01-31"] }
        ]))[0],
    );
    assert_eq!(
        values,
        vec![
            Some("2026-01-01".to_string()),
            Some("2026-01-31".to_string())
        ]
    );
}

#[test]
fn absolute_offset_timestamps_pass_through_unchanged() {
    for pair in [
        ["2026-01-01T00:00:00Z", "2026-01-31T23:59:59Z"],
        ["2026-01-01T00:00:00+03:00", "2026-01-31T23:59:59+03:00"],
    ] {
        let values = leaf_values(
            &filters_of(json!([
                { "member": "Orders.createdAt", "operator": "inDateRange", "values": pair }
            ]))[0],
        );
        assert_eq!(
            values,
            vec![Some(pair[0].to_string()), Some(pair[1].to_string())]
        );
    }
}

#[test]
fn single_date_operators_pick_the_matching_bound() {
    for (operator, suffix) in [
        ("beforeDate", "T00:00:00.000"),
        ("afterOrOnDate", "T00:00:00.000"),
        ("beforeOrOnDate", "T23:59:59.999"),
        ("afterDate", "T23:59:59.999"),
    ] {
        let values = leaf_values(
            &filters_of(json!([
                { "member": "Orders.createdAt", "operator": operator, "values": ["last 2 weeks"] }
            ]))[0],
        );
        assert_eq!(values.len(), 1, "{operator}");
        let value = values[0].clone().unwrap();
        assert!(value.ends_with(suffix), "{operator}: {value}");
    }
}

#[test]
fn on_the_date_resolves_to_a_two_value_range() {
    let values = leaf_values(
        &filters_of(json!([
            { "member": "Orders.createdAt", "operator": "onTheDate", "values": ["yesterday"] }
        ]))[0],
    );
    assert_eq!(values.len(), 2);
}

#[test]
fn absolute_values_pass_through_byte_exact() {
    for value in ["2026-01-15", "2026-01-15T10:30:00.000"] {
        let values = leaf_values(
            &filters_of(json!([
                { "member": "Orders.createdAt", "operator": "beforeDate", "values": [value] }
            ]))[0],
        );
        assert_eq!(values, vec![Some(value.to_string())]);
    }
}

#[test]
fn values_are_stringified_and_non_date_operators_untouched() {
    let values = leaf_values(
        &filters_of(json!([
            { "member": "Orders.status", "operator": "equals", "values": ["shipped", 5, true, null] }
        ]))[0],
    );
    assert_eq!(
        values,
        vec![
            Some("shipped".to_string()),
            Some("5".to_string()),
            Some("true".to_string()),
            None
        ]
    );
}

#[test]
fn dimension_is_the_legacy_alias_of_member() {
    match &filters_of(json!([
        { "dimension": "Orders.status", "operator": "equals", "values": ["x"] }
    ]))[0]
    {
        NormalizedFilter::Leaf(leaf) => assert_eq!(leaf.member, "Orders.status"),
        other => panic!("expected a leaf, got {:?}", other),
    }
}

#[test]
fn nested_groups_are_resolved_recursively() {
    let filters = filters_of(json!([
        {
            "and": [
                { "or": [
                    { "member": "Orders.createdAt", "operator": "inDateRange", "values": ["last 2 weeks"] },
                    { "member": "Orders.status", "operator": "equals", "values": ["shipped"] }
                ] }
            ]
        }
    ]));

    let NormalizedFilter::And(and) = &filters[0] else {
        panic!("expected an AND group");
    };
    let NormalizedFilter::Or(or) = &and[0] else {
        panic!("expected an OR group");
    };
    assert_eq!(leaf_values(&or[0]).len(), 2, "date filter was resolved");
    assert_eq!(leaf_values(&or[1]), vec![Some("shipped".to_string())]);
}

#[test]
fn range_operator_with_several_relative_values_is_rejected() {
    for operator in ["inDateRange", "notInDateRange"] {
        let filters: Vec<QueryFilter> = serde_json::from_value(json!([
            { "member": "Orders.createdAt", "operator": operator, "values": ["last 2 weeks", "yesterday"] }
        ]))
        .unwrap();
        let err = normalize_query_filters(&filters, "UTC").unwrap_err();
        assert!(err.is_user_error());
        assert!(
            err.message()
                .contains("only supported when `values` has a single element"),
            "{}",
            err.message()
        );
    }
}

#[test]
fn values_are_required_except_for_set_like_operators() {
    let filters: Vec<QueryFilter> = serde_json::from_value(json!([
        { "member": "Orders.status", "operator": "equals", "values": [] }
    ]))
    .unwrap();
    let err = normalize_query_filters(&filters, "UTC").unwrap_err();
    assert!(err.message().starts_with("Values required for filter:"));

    for operator in ["set", "notSet", "measureFilter"] {
        let filters: Vec<QueryFilter> = serde_json::from_value(json!([
            { "member": "Orders.status", "operator": operator }
        ]))
        .unwrap();
        assert!(
            normalize_query_filters(&filters, "UTC").is_ok(),
            "{operator}"
        );
    }
}

#[test]
fn invalid_relative_date_is_a_user_error() {
    let filters: Vec<QueryFilter> = serde_json::from_value(json!([
        { "member": "Orders.createdAt", "operator": "inDateRange", "values": ["not a date"] }
    ]))
    .unwrap();
    let err = normalize_query_filters(&filters, "UTC").unwrap_err();
    assert!(err.is_user_error(), "{}", err.message());
}

// ------------------------------------------------------------- normalize

#[test]
fn requires_measures_dimensions_or_a_granularity() {
    let err = normalize(json!({ "timezone": "UTC" })).unwrap_err();
    assert_eq!(
        err.message(),
        "Query should contain either measures, dimensions or timeDimensions with granularities in order to be valid"
    );

    let err = normalize(json!({
        "timeDimensions": [{ "dimension": "Orders.createdAt", "dateRange": "last 2 weeks" }]
    }))
    .unwrap_err();
    assert!(err.is_user_error());

    assert!(normalize(json!({
        "timeDimensions": [{ "dimension": "Orders.createdAt", "granularity": "day" }]
    }))
    .is_ok());
}

#[test]
fn keeps_an_explicit_limit_of_zero_and_applies_the_default_otherwise() {
    let mut query = base();
    query["limit"] = json!(0);
    assert_eq!(normalize(query).unwrap().limit, Some(0));

    let normalized = normalize(base()).unwrap();
    assert_eq!(normalized.limit, Some(config().effective_default_limit()));
    assert_eq!(normalized.row_limit, normalized.limit);
}

#[test]
fn rejects_a_limit_over_the_hard_cap_unless_persistent() {
    let mut query = base();
    query["limit"] = json!(config().db_query_limit + 1);

    let err = normalize(query.clone()).unwrap_err();
    assert_eq!(err.message(), "The query limit has been exceeded.");
    // Node throws a plain Error here, i.e. HTTP 500, not a UserError.
    assert!(!err.is_user_error());

    let normalized = normalize_query(&query_from(query), true, None, &config()).unwrap();
    assert_eq!(normalized.limit, Some(config().db_query_limit + 1));
}

#[test]
fn persistent_queries_keep_a_missing_limit_unset() {
    let normalized = normalize_query(&query_from(base()), true, None, &config()).unwrap();
    assert_eq!(normalized.limit, None);
}

#[test]
fn canonicalizes_the_timezone_and_falls_back_to_the_default() {
    let mut query = base();
    query["timezone"] = json!("america/new_york");
    assert_eq!(normalize(query).unwrap().timezone, "America/New_York");

    let mut query = base();
    query.as_object_mut().unwrap().remove("timezone");
    assert_eq!(normalize(query.clone()).unwrap().timezone, "UTC");

    let config = QueryConfig {
        default_timezone: "America/Sao_Paulo".to_string(),
        ..QueryConfig::default()
    };
    assert_eq!(
        normalize_query(&query_from(query), false, None, &config)
            .unwrap()
            .timezone,
        "America/Sao_Paulo"
    );
}

#[test]
fn rejects_an_unknown_timezone() {
    let mut query = base();
    query["timezone"] = json!("Not/AZone");
    let err = normalize(query).unwrap_err();
    assert!(
        err.message().contains("Invalid query format"),
        "{}",
        err.message()
    );
}

#[test]
fn dimensions_with_a_granularity_become_time_dimensions() {
    let mut query = base();
    query["dimensions"] = json!(["Orders.createdAt.week", "Orders.status"]);

    let normalized = normalize(query).unwrap();
    assert_eq!(
        normalized.dimensions,
        vec![QueryMember::Name("Orders.status".to_string())]
    );
    assert_eq!(normalized.time_dimensions.len(), 1);
    assert_eq!(normalized.time_dimensions[0].dimension, "Orders.createdAt");
    assert_eq!(
        normalized.time_dimensions[0].granularity.as_deref(),
        Some("week")
    );
}

#[test]
fn order_is_remapped_to_id_and_desc() {
    let mut query = base();
    query["order"] = json!({ "Orders.count": "desc", "Orders.status": "asc" });
    let normalized = normalize(query).unwrap();
    assert_eq!(
        normalized.order,
        Some(vec![
            OrderItem {
                id: "Orders.count".to_string(),
                desc: true
            },
            OrderItem {
                id: "Orders.status".to_string(),
                desc: false
            },
        ])
    );

    let mut query = base();
    query["order"] = json!([["Orders.count", "desc"]]);
    assert_eq!(
        normalize(query).unwrap().order,
        Some(vec![OrderItem {
            id: "Orders.count".to_string(),
            desc: true
        }])
    );
}

#[test]
fn time_dimension_date_ranges_are_resolved() {
    let mut query = base();
    query["timeDimensions"] = json!([
        { "dimension": "Orders.createdAt", "granularity": "day", "dateRange": ["2026-01-01", "2026-01-31"] }
    ]);
    let normalized = normalize(query).unwrap();
    assert_eq!(
        normalized.time_dimensions[0].date_range,
        Some([
            "2026-01-01T00:00:00.000".to_string(),
            "2026-01-31T23:59:59.999".to_string()
        ])
    );

    let mut query = base();
    query["timeDimensions"] = json!([
        { "dimension": "Orders.createdAt", "granularity": "day", "dateRange": ["2026-01-01"] }
    ]);
    assert_eq!(
        normalize(query).unwrap().time_dimensions[0].date_range,
        Some([
            "2026-01-01T00:00:00.000".to_string(),
            "2026-01-01T23:59:59.999".to_string()
        ])
    );
}

#[test]
fn cache_mode_defaults_to_stale_if_slow() {
    assert_eq!(
        normalize(base()).unwrap().cache_mode,
        CacheMode::StaleIfSlow
    );

    let mut query = base();
    query["cache"] = json!("no-cache");
    assert_eq!(normalize(query).unwrap().cache_mode, CacheMode::NoCache);

    let mut query = base();
    query["cache"] = json!("no-cache");
    assert_eq!(
        normalize_query(
            &query_from(query),
            false,
            Some(CacheMode::MustRevalidate),
            &config()
        )
        .unwrap()
        .cache_mode,
        CacheMode::MustRevalidate
    );
}

#[test]
fn rejects_unknown_fields_and_cache_modes() {
    let mut query = base();
    query["nope"] = json!(1);
    assert!(serde_json::from_value::<Query>(query).is_err());

    let mut query = base();
    query["cache"] = json!("sometimes");
    assert!(serde_json::from_value::<Query>(query).is_err());
}

// --------------------------------------------------------------- request

#[test]
fn parse_query_param_requires_a_query() {
    for value in [Value::Null, json!(""), json!("undefined")] {
        let err = parse_query_param(&value).unwrap_err();
        assert_eq!(err.message(), "Query param is required");
    }
}

#[test]
fn parse_query_param_decodes_json_strings_and_arrays() {
    let queries = parse_query_param(&json!(r#"{"measures":["Orders.count"]}"#)).unwrap();
    assert_eq!(queries.len(), 1);

    let queries = parse_query_param(&json!(
        r#"[{"measures":["Orders.count"]},{"measures":["Orders.total"]}]"#
    ))
    .unwrap();
    assert_eq!(queries.len(), 2);

    let queries = parse_query_param(&json!([{ "measures": ["Orders.count"] }])).unwrap();
    assert_eq!(queries.len(), 1);

    let err = parse_query_param(&json!("{ nope")).unwrap_err();
    assert!(err
        .message()
        .starts_with("Unable to decode query param as JSON"));
}

#[test]
fn compare_date_range_fans_the_query_out() {
    let query = query_from(json!({
        "measures": ["Orders.count"],
        "timeDimensions": [{
            "dimension": "Orders.createdAt",
            "granularity": "day",
            "compareDateRange": [["2026-01-01", "2026-01-31"], ["2026-02-01", "2026-02-28"]]
        }]
    }));

    let expanded = compare_date_range_transformer(&query).unwrap();
    assert_eq!(expanded.len(), 2);
    for expanded_query in &expanded {
        let td = &expanded_query.time_dimensions.as_ref().unwrap()[0];
        assert!(td.compare_date_range.is_none());
        assert!(td.date_range.is_some());
    }
}

#[test]
fn compare_date_range_is_allowed_on_one_time_dimension_only() {
    let query = query_from(json!({
        "measures": ["Orders.count"],
        "timeDimensions": [
            { "dimension": "Orders.createdAt", "compareDateRange": [["2026-01-01", "2026-01-31"]] },
            { "dimension": "Orders.updatedAt", "compareDateRange": [["2026-01-01", "2026-01-31"]] }
        ]
    }));
    let err = compare_date_range_transformer(&query).unwrap_err();
    assert_eq!(
        err.message(),
        "compareDateRange can only exist for one timeDimension"
    );
}

#[test]
fn query_type_follows_the_request_shape() {
    let (query_type, queries) = get_normalized_queries(&base(), false, None, &config()).unwrap();
    assert_eq!(query_type, QueryType::RegularQuery);
    assert_eq!(queries.len(), 1);

    let (query_type, queries) = get_normalized_queries(
        &json!([base(), { "measures": ["Orders.total"], "timezone": "UTC" }]),
        false,
        None,
        &config(),
    )
    .unwrap();
    assert_eq!(query_type, QueryType::BlendingQuery);
    assert_eq!(queries.len(), 2);

    let (query_type, queries) = get_normalized_queries(
        &json!({
            "measures": ["Orders.count"],
            "timeDimensions": [{
                "dimension": "Orders.createdAt",
                "granularity": "day",
                "compareDateRange": [["2026-01-01", "2026-01-31"], ["2026-02-01", "2026-02-28"]]
            }]
        }),
        false,
        None,
        &config(),
    )
    .unwrap();
    assert_eq!(query_type, QueryType::CompareDateRangeQuery);
    assert_eq!(queries.len(), 2);
}

#[test]
fn pivot_query_mirrors_the_query_type() {
    let (query_type, queries) = get_normalized_queries(
        &json!([
            { "measures": ["Orders.count"], "dimensions": ["Orders.status"], "timeDimensions": [{ "dimension": "Orders.createdAt", "granularity": "day" }], "timezone": "UTC" },
            { "measures": ["Orders.total"], "dimensions": ["Orders.status"], "timeDimensions": [{ "dimension": "Orders.createdAt", "granularity": "day" }], "timezone": "UTC" }
        ]),
        false,
        None,
        &config(),
    )
    .unwrap();

    let PivotQuery::Blending(pivot) = get_pivot_query(query_type, &queries).unwrap() else {
        panic!("expected a blending pivot query");
    };
    assert_eq!(pivot.measures.len(), 2);
    assert_eq!(pivot.dimensions.len(), 1);
    assert_eq!(pivot.time_dimensions[0].dimension, "time");
    assert_eq!(pivot.time_dimensions[0].granularity.as_deref(), Some("day"));

    let (query_type, queries) = get_normalized_queries(&base(), false, None, &config()).unwrap();
    let PivotQuery::Query(pivot) = get_pivot_query(query_type, &queries).unwrap() else {
        panic!("expected a plain pivot query");
    };
    assert_eq!(pivot.query_type, Some(QueryType::RegularQuery));
}
