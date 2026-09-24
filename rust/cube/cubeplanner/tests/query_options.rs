//! The query options beyond the members themselves: join hints, masking,
//! pre-aggregation selection and the request-level switches. Each test shows
//! the option changing the SQL, not just being accepted.

use cubeplanner::{plan, Dialect, Model, PlanOptions, PlannedSql, PlannerQuery};
use serde_json::json;

fn test_model() -> Model {
    Model::from_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/model")).unwrap()
}

/// The planner's own fixture with three rollups on `visitors`.
fn pre_aggregation_model() -> Model {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../cubesqlplanner/cubesqlplanner/src/test_fixtures/schemas/yaml_files/common/\
         pre_aggregations_test.yaml"
    );
    Model::from_yaml_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn plan_on(model: &Model, value: serde_json::Value) -> PlannedSql {
    let query = PlannerQuery::from_value(value).unwrap_or_else(|e| panic!("{e}"));
    plan(model, &query, &PlanOptions::postgres()).unwrap_or_else(|e| panic!("{e}"))
}

fn plan_query(value: serde_json::Value) -> PlannedSql {
    plan_on(&test_model(), value)
}

/// The rollup query every pre-aggregation test below varies.
fn rollup_query() -> serde_json::Value {
    json!({
        "measures": ["visitors.count"],
        "dimensions": ["visitors.source"],
        "timeDimensions": [{ "dimension": "visitors.created_at", "granularity": "day" }]
    })
}

fn with(mut query: serde_json::Value, key: &str, value: serde_json::Value) -> serde_json::Value {
    query
        .as_object_mut()
        .unwrap()
        .insert(key.to_string(), value);
    query
}

// ---- joinHints -------------------------------------------------------------

/// A hint joins a cube the query's own members never mention.
#[test]
fn join_hints_add_a_join_the_members_do_not_need() {
    let without = plan_query(json!({ "measures": ["orders.count"] }));
    assert!(!without.sql.contains("LEFT JOIN"), "{}", without.sql);

    let hinted = plan_query(json!({
        "measures": ["orders.count"],
        "joinHints": ["users"]
    }));
    assert!(hinted.sql.contains("LEFT JOIN"), "{}", hinted.sql);
    assert!(hinted.sql.contains("public.users"), "{}", hinted.sql);
    insta::assert_snapshot!(hinted.sql);
}

/// A hint may also be a whole join path, which is how the SQL API sends one.
#[test]
fn a_join_hint_may_be_a_path() {
    let hinted = plan_query(json!({
        "measures": ["orders.count"],
        "joinHints": [["orders", "users"]]
    }));
    assert!(hinted.sql.contains("LEFT JOIN"), "{}", hinted.sql);
}

// ---- maskedMembers ---------------------------------------------------------

/// A masked member renders its `mask:` instead of its `sql:`.
#[test]
fn masked_members_render_their_mask() {
    let query = json!({
        "measures": ["users.count"],
        "dimensions": ["users.city_masked"]
    });

    let plain = plan_query(query.clone());
    assert!(plain.sql.contains(r#""users".city"#), "{}", plain.sql);

    let masked = plan_query(with(
        query,
        "maskedMembers",
        json!([{ "member": "users.city_masked", "filter": null }]),
    ));
    assert!(masked.sql.contains("'***'"), "{}", masked.sql);
    assert!(!masked.sql.contains(r#""users".city "#), "{}", masked.sql);
    insta::assert_snapshot!(masked.sql);
}

/// A masked member carrying a filter shows its real value only for the rows
/// that filter matches, so the mask becomes a `CASE`.
#[test]
fn a_masked_member_with_a_filter_becomes_a_case() {
    let released = plan_query(json!({
        "measures": ["users.count"],
        "dimensions": ["users.city_masked"],
        "filters": [{ "member": "users.city", "operator": "equals", "values": ["Berlin"] }],
        "maskedMembers": [{
            "member": "users.city_masked",
            "filter": { "member": "users.city", "operator": "equals", "values": ["Berlin"] }
        }]
    }));

    assert!(
        released
            .sql
            .contains(r#"CASE WHEN ("users".city = $1) THEN "users".city ELSE '***' END"#),
        "{}",
        released.sql
    );
    insta::assert_snapshot!(released.sql);
}

// ---- pre-aggregations ------------------------------------------------------

/// Without any of the switches below, the query is served from the rollup.
#[test]
fn a_matching_rollup_is_used() {
    let planned = plan_on(&pre_aggregation_model(), rollup_query());
    assert!(
        planned.sql.contains("visitors__daily_rollup"),
        "{}",
        planned.sql
    );
    insta::assert_snapshot!(planned.sql);
}

/// A query that *builds* a pre-aggregation is never itself served from one.
#[test]
fn pre_aggregation_query_reads_the_cube_instead_of_a_rollup() {
    let planned = plan_on(
        &pre_aggregation_model(),
        with(rollup_query(), "preAggregationQuery", json!(true)),
    );
    assert!(
        !planned.sql.contains("visitors__daily_rollup"),
        "{}",
        planned.sql
    );
    assert!(planned.sql.contains("FROM  visitors"), "{}", planned.sql);
    insta::assert_snapshot!(planned.sql);
}

/// The fixture's rollups are external (the JS default), so disabling external
/// pre-aggregations leaves nothing to match.
#[test]
fn disable_external_pre_aggregations_falls_back_to_the_cube() {
    let planned = plan_on(
        &pre_aggregation_model(),
        with(
            rollup_query(),
            "disableExternalPreAggregations",
            json!(true),
        ),
    );
    assert!(
        !planned.sql.contains("visitors__daily_rollup"),
        "{}",
        planned.sql
    );
}

/// `preAggregationId` pins the match to one pre-aggregation: the right one is
/// used, and a different one leaves the query on the cube.
#[test]
fn pre_aggregation_id_pins_the_match() {
    let pinned = plan_on(
        &pre_aggregation_model(),
        with(
            rollup_query(),
            "preAggregationId",
            json!("visitors.daily_rollup"),
        ),
    );
    assert!(
        pinned.sql.contains("visitors__daily_rollup"),
        "{}",
        pinned.sql
    );

    let other = plan_on(
        &pre_aggregation_model(),
        with(
            rollup_query(),
            "preAggregationId",
            json!("visitors.for_join"),
        ),
    );
    assert!(
        !other.sql.contains("visitors__daily_rollup"),
        "{}",
        other.sql
    );
}

/// An external rollup is read through the external dialect, so the placeholder
/// style switches with it.
#[test]
fn an_external_rollup_renders_in_the_external_dialect() {
    let query = PlannerQuery::from_value(with(
        rollup_query(),
        "filters",
        json!([{ "member": "visitors.source", "operator": "equals", "values": ["google"] }]),
    ))
    .unwrap();

    let options = PlanOptions::postgres().with_external_dialect(Dialect::CubeStore);
    let planned = plan(&pre_aggregation_model(), &query, &options).unwrap();

    assert!(
        planned.sql.contains("visitors__daily_rollup"),
        "{}",
        planned.sql
    );
    assert!(planned.sql.contains('?'), "{}", planned.sql);
    assert!(!planned.sql.contains("$1"), "{}", planned.sql);
}

// ---- request-level switches ------------------------------------------------

/// `totalQuery` wraps the query in a count of its rows.
#[test]
fn total_query_counts_the_rows() {
    let planned = plan_query(json!({
        "measures": ["orders.count"],
        "dimensions": ["orders.status"],
        "totalQuery": true
    }));

    assert!(planned.sql.contains("COUNT(*)"), "{}", planned.sql);
    insta::assert_snapshot!(planned.sql);
}

/// `exportAnnotatedSql` keeps the parameter's own index in the SQL (`$0$`)
/// instead of rendering the dialect's placeholder, so the caller can map each
/// one back to the member it came from.
#[test]
fn export_annotated_sql_keeps_the_parameter_index() {
    let query = json!({
        "measures": ["orders.count"],
        "filters": [{ "member": "orders.status", "operator": "equals", "values": ["new"] }]
    });

    let plain = plan_query(query.clone());
    assert!(plain.sql.contains("= $1"), "{}", plain.sql);

    let annotated = plan_query(with(query.clone(), "exportAnnotatedSql", json!(true)));
    assert!(annotated.sql.contains("= $0$"), "{}", annotated.sql);

    // The same switch is also a request-level default.
    let from_options = plan(
        &test_model(),
        &PlannerQuery::from_value(query).unwrap(),
        &PlanOptions::postgres().with_export_annotated_sql(true),
    )
    .unwrap();
    assert_eq!(from_options.sql, annotated.sql);
}

/// `convertTzForRawTimeDimension` converts a time dimension selected without a
/// granularity into the query's timezone.
#[test]
fn convert_tz_for_raw_time_dimension_converts_the_raw_column() {
    let query = json!({
        "measures": ["orders.count"],
        "dimensions": ["orders.created_at"],
        "timezone": "America/Los_Angeles"
    });

    let plain = plan_query(query.clone());
    assert!(
        !plain.sql.contains("AT TIME ZONE 'America/Los_Angeles'"),
        "{}",
        plain.sql
    );

    let converted = plan_query(with(
        query.clone(),
        "convertTzForRawTimeDimension",
        json!(true),
    ));
    assert!(
        converted.sql.contains("AT TIME ZONE 'America/Los_Angeles'"),
        "{}",
        converted.sql
    );
    insta::assert_snapshot!(converted.sql);

    // The same switch is also a request-level default.
    let from_options = plan(
        &test_model(),
        &PlannerQuery::from_value(query).unwrap(),
        &PlanOptions::postgres().with_convert_tz_for_raw_time_dimension(true),
    )
    .unwrap();
    assert_eq!(from_options.sql, converted.sql);
}

/// `ungrouped` returns the joined rows instead of aggregating them.
#[test]
fn ungrouped_drops_the_group_by() {
    let planned = plan_query(json!({
        "dimensions": ["orders.status", "users.city"],
        "ungrouped": true,
        "limit": 5
    }));
    assert!(!planned.sql.contains("GROUP BY"), "{}", planned.sql);
}

/// `cubestoreSupportMultistage` reaches the pre-aggregation optimizer. It
/// defaults to `false`, like `BaseQuery.try_new`, and a plain query is
/// unaffected either way.
#[test]
fn cubestore_support_multistage_is_accepted_and_defaults_to_false() {
    let query: PlannerQuery = PlannerQuery::from_value(rollup_query()).unwrap();
    assert_eq!(query.cubestore_support_multistage, None);

    let model = pre_aggregation_model();
    let default = plan_on(&model, rollup_query());
    let enabled = plan_on(
        &model,
        with(rollup_query(), "cubestoreSupportMultistage", json!(true)),
    );
    assert_eq!(default.sql, enabled.sql);
}

/// Every option is part of the request body, so it round-trips through JSON.
#[test]
fn the_whole_option_set_deserializes_from_one_request_body() {
    let query = PlannerQuery::from_json(
        r#"{
            "measures": ["orders.count"],
            "dimensions": ["orders.status"],
            "joinHints": ["users", ["orders", "users"]],
            "subqueryJoins": [],
            "maskedMembers": [{ "member": "users.city_masked", "filter": null }],
            "preAggregationId": "orders.main",
            "preAggregationQuery": true,
            "disableExternalPreAggregations": true,
            "cubestoreSupportMultistage": true,
            "exportAnnotatedSql": true,
            "convertTzForRawTimeDimension": true,
            "totalQuery": true,
            "ungrouped": false
        }"#,
    )
    .unwrap();

    assert_eq!(query.join_hints.len(), 2);
    assert_eq!(query.masked_members.len(), 1);
    assert_eq!(query.pre_aggregation_id.as_deref(), Some("orders.main"));
    assert_eq!(query.pre_aggregation_query, Some(true));
    assert_eq!(query.disable_external_pre_aggregations, Some(true));
    assert_eq!(query.cubestore_support_multistage, Some(true));
    assert_eq!(query.export_annotated_sql, Some(true));
    assert_eq!(query.convert_tz_for_raw_time_dimension, Some(true));
    assert_eq!(query.total_query, Some(true));
    assert_eq!(query.ungrouped, Some(false));
}
