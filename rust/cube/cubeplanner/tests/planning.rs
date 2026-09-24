//! End-to-end planning: YAML model + normalized REST query -> Postgres SQL,
//! with no JavaScript anywhere in the pipeline.

use cubeplanner::{plan, Dialect, Model, PlanOptions, PlannedSql, PlannerQuery};
use serde_json::json;

/// The planner's own YAML fixtures, reused here so both crates exercise the
/// same models.
fn fixture(relative: &str) -> Model {
    let path = format!(
        "{}/../cubesqlplanner/cubesqlplanner/src/test_fixtures/schemas/yaml_files/{}",
        env!("CARGO_MANIFEST_DIR"),
        relative
    );
    let yaml = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    Model::from_yaml_str(&yaml).unwrap_or_else(|e| panic!("load {path}: {e}"))
}

/// The model written for these tests: two joined cubes, a cube `sql` using
/// `FILTER_PARAMS` and `SECURITY_CONTEXT`, a custom granularity, a filtered
/// measure and a segment.
fn test_model() -> Model {
    Model::from_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/model")).unwrap()
}

fn query(value: serde_json::Value) -> PlannerQuery {
    PlannerQuery::from_value(value).unwrap()
}

fn plan_query(model: &Model, value: serde_json::Value) -> PlannedSql {
    plan(model, &query(value), &PlanOptions::postgres()).unwrap_or_else(|e| panic!("{e}"))
}

fn plan_with(model: &Model, value: serde_json::Value, options: &PlanOptions) -> PlannedSql {
    plan(model, &query(value), options).unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn model_from_dir_merges_files() {
    assert_eq!(test_model().cube_names(), vec!["orders", "sales", "users"]);
}

#[test]
fn join_graph_finds_the_join_path() {
    use cubeplanner::join_graph::{JoinDefinition, JoinHintItem};

    let model = test_model();
    let graph = model.join_graph().unwrap();
    let join = graph
        .build_join(vec![
            JoinHintItem::Single("orders".to_string()),
            JoinHintItem::Single("users".to_string()),
        ])
        .unwrap();

    assert_eq!(join.static_data().root, "orders");
    assert_eq!(
        join.joins()
            .unwrap()
            .iter()
            .map(|item| item.static_data().to.clone())
            .collect::<Vec<_>>(),
        vec!["users".to_string()]
    );
}

#[test]
fn view_members_resolve_to_their_cubes() {
    let planned = plan_query(
        &test_model(),
        json!({
            "measures": ["sales.total_amount"],
            "dimensions": ["sales.users_city"],
            "order": [{ "id": "sales.total_amount", "desc": true }]
        }),
    );

    insta::assert_snapshot!(planned.sql);
}

#[test]
fn sql_utils_convert_tz_uses_the_query_timezone() {
    let planned = plan_with(
        &test_model(),
        json!({
            "measures": ["orders.count"],
            "dimensions": ["orders.created_at_local"],
            "timezone": "Europe/Berlin"
        }),
        &PlanOptions::postgres(),
    );

    assert!(
        planned.sql.contains("AT TIME ZONE 'Europe/Berlin'"),
        "{}",
        planned.sql
    );
    insta::assert_snapshot!(planned.sql);
}

#[test]
fn simple_aggregation() {
    let planned = plan_query(
        &test_model(),
        json!({ "measures": ["orders.count"], "dimensions": ["orders.status"] }),
    );

    insta::assert_snapshot!(planned.sql);
}

#[test]
fn join_across_cubes() {
    let planned = plan_query(
        &test_model(),
        json!({
            "measures": ["orders.total_amount"],
            "dimensions": ["users.city"],
            "order": [{ "id": "orders.total_amount", "desc": true }],
            "limit": 10
        }),
    );

    assert!(planned.sql.contains("LEFT JOIN"), "{}", planned.sql);
    insta::assert_snapshot!(planned.sql);
}

#[test]
fn time_dimension_with_granularity_and_date_range() {
    let planned = plan_query(
        &test_model(),
        json!({
            "measures": ["orders.count"],
            "timeDimensions": [{
                "dimension": "orders.created_at",
                "granularity": "month",
                "dateRange": ["2024-01-01", "2024-03-31"]
            }]
        }),
    );

    assert!(planned.sql.contains("date_trunc"), "{}", planned.sql);
    insta::assert_snapshot!(planned.sql);
}

#[test]
fn custom_granularity() {
    let planned = plan_query(
        &test_model(),
        json!({
            "measures": ["orders.count"],
            "timeDimensions": [{
                "dimension": "orders.created_at",
                "granularity": "half_year"
            }]
        }),
    );

    insta::assert_snapshot!(planned.sql);
}

#[test]
fn filters_and_segments_become_parameters() {
    let planned = plan_query(
        &test_model(),
        json!({
            "measures": ["orders.count"],
            "segments": ["orders.completed"],
            "filters": [{
                "member": "users.city",
                "operator": "equals",
                "values": ["Berlin", "Paris"]
            }]
        }),
    );

    assert_eq!(
        planned.param_strings(),
        vec![Some("Berlin".to_string()), Some("Paris".to_string())]
    );
    insta::assert_snapshot!(planned.sql);
}

#[test]
fn boolean_filter_groups() {
    let planned = plan_query(
        &test_model(),
        json!({
            "measures": ["orders.count"],
            "filters": [{
                "or": [
                    { "member": "orders.status", "operator": "equals", "values": ["shipped"] },
                    { "and": [
                        { "member": "orders.status", "operator": "equals", "values": ["new"] },
                        { "member": "users.city", "operator": "set" }
                    ]}
                ]
            }]
        }),
    );

    insta::assert_snapshot!(planned.sql);
}

#[test]
fn filter_params_push_the_date_range_into_the_cube_sql() {
    let model = test_model();

    let without_range = plan_query(&model, json!({ "measures": ["orders.count"] }));
    assert!(
        !without_range.sql.contains("created_at >="),
        "an unfiltered query must not push a range down: {}",
        without_range.sql
    );

    let with_range = plan_query(
        &model,
        json!({
            "measures": ["orders.count"],
            "timeDimensions": [{
                "dimension": "orders.created_at",
                "dateRange": ["2024-01-01", "2024-01-31"]
            }]
        }),
    );

    assert!(
        with_range.sql.contains("created_at >="),
        "FILTER_PARAMS should push the range into the cube sql: {}",
        with_range.sql
    );
    insta::assert_snapshot!(with_range.sql);
}

#[test]
fn security_context_is_resolved_per_request() {
    let model = test_model();
    let request = json!({ "measures": ["orders.count"] });

    let anonymous = plan_query(&model, request.clone());
    assert!(anonymous.sql.contains("1 = 1"), "{}", anonymous.sql);

    let scoped = plan_with(
        &model,
        request,
        &PlanOptions::postgres().with_security_context(json!({ "tenant_id": 42 })),
    );
    assert!(scoped.sql.contains("tenant_id = $1"), "{}", scoped.sql);
    assert_eq!(scoped.param_strings(), vec![Some("42".to_string())]);
    insta::assert_snapshot!(scoped.sql);
}

#[test]
fn filtered_measure() {
    let planned = plan_query(
        &test_model(),
        json!({ "measures": ["orders.completed_amount"] }),
    );

    insta::assert_snapshot!(planned.sql);
}

#[test]
fn ungrouped_query() {
    let planned = plan_query(
        &test_model(),
        json!({
            "dimensions": ["orders.status", "users.city"],
            "ungrouped": true,
            "limit": 5
        }),
    );

    assert!(!planned.sql.contains("GROUP BY"), "{}", planned.sql);
    insta::assert_snapshot!(planned.sql);
}

#[test]
fn timezone_shifts_the_time_dimension() {
    let planned = plan_with(
        &test_model(),
        json!({
            "measures": ["orders.count"],
            "timeDimensions": [{ "dimension": "orders.created_at", "granularity": "day" }],
            "timezone": "America/Los_Angeles"
        }),
        &PlanOptions::postgres(),
    );

    assert!(
        planned.sql.contains("America/Los_Angeles"),
        "{}",
        planned.sql
    );
}

/// `/v1/load` answers by member, the SQL selects by alias, so every planned
/// query carries the map between them.
#[test]
fn the_result_columns_map_back_to_their_members() {
    let planned = plan_query(
        &test_model(),
        json!({
            "measures": ["orders.count"],
            "dimensions": ["orders.status"],
            "timeDimensions": [{
                "dimension": "orders.created_at",
                "granularity": "month"
            }]
        }),
    );

    let mut mapping: Vec<(String, String)> = planned
        .alias_name_to_member
        .iter()
        .map(|(alias, member)| (alias.clone(), member.clone()))
        .collect();
    mapping.sort();

    assert_eq!(
        mapping,
        vec![
            ("orders__count".to_string(), "orders.count".to_string()),
            (
                "orders__created_at_month".to_string(),
                "orders.created_at.month".to_string()
            ),
            ("orders__status".to_string(), "orders.status".to_string()),
        ]
    );

    // Every alias the map names is really a column of the generated SQL.
    for alias in planned.alias_name_to_member.keys() {
        assert!(
            planned.sql.contains(&format!(r#""{alias}""#)),
            "`{alias}` is not selected by: {}",
            planned.sql
        );
    }
}

/// A time dimension asked for without a granularity is not keyed by one, the
/// way `BaseQuery.aliasNameToMember` leaves it out.
#[test]
fn a_raw_time_dimension_is_mapped_as_a_plain_dimension() {
    let planned = plan_query(
        &test_model(),
        json!({
            "measures": ["orders.count"],
            "dimensions": ["orders.created_at"]
        }),
    );

    assert_eq!(
        planned.alias_name_to_member.get("orders__created_at"),
        Some(&"orders.created_at".to_string())
    );
}

/// A view's members are named by the view, not by the cube they came from.
#[test]
fn view_members_map_back_to_the_view() {
    let planned = plan_query(
        &test_model(),
        json!({
            "measures": ["sales.total_amount"],
            "dimensions": ["sales.users_city"]
        }),
    );

    assert_eq!(
        planned.alias_name_to_member.get("sales__users_city"),
        Some(&"sales.users_city".to_string()),
        "{:?}",
        planned.alias_name_to_member
    );
}

// ---- the planner's own fixtures -------------------------------------------

#[test]
fn plans_the_planner_fixture_model() {
    let planned = plan_query(
        &fixture("common/visitors.yaml"),
        json!({
            "measures": ["visitors.count"],
            "dimensions": ["visitors.source"],
            "timeDimensions": [{ "dimension": "visitors.created_at", "granularity": "day" }],
            "order": [["visitors.count", "desc"]],
            "limit": 25
        }),
    );

    assert!(planned.sql.contains("ORDER BY"), "{}", planned.sql);
    assert!(planned.sql.contains("LIMIT 25"), "{}", planned.sql);
    insta::assert_snapshot!(planned.sql);
}

#[test]
fn plans_a_join_from_the_planner_fixture_model() {
    let planned = plan_query(
        &fixture("common/diamond_joins.yaml"),
        json!({ "measures": ["cube_a.count"], "dimensions": ["cube_c.code"] }),
    );

    assert!(
        planned.sql.contains(r#"ON "cube_a".c_id = "cube_c".id"#) || planned.sql.contains("cube_c"),
        "{}",
        planned.sql
    );
    insta::assert_snapshot!(planned.sql);
}

/// A calc-group view declares members of its own — a `switch` dimension and a
/// multi-stage `case` dimension that fans out over it — instead of including
/// cube members.
#[test]
fn plans_a_calc_group_view_that_declares_its_own_dimensions() {
    let planned = plan_query(
        &fixture("common/calc_groups_cross_join.yaml"),
        json!({
            "dimensions": ["source.product_category"],
            "order": [{ "id": "source.product_category" }]
        }),
    );

    assert!(
        planned.sql.contains("calc_groups_source_a"),
        "{}",
        planned.sql
    );
    assert!(
        planned.sql.contains("calc_groups_source_b"),
        "{}",
        planned.sql
    );
    insta::assert_snapshot!(planned.sql);
}

// ---- errors ----------------------------------------------------------------

#[test]
fn unknown_member_is_reported() {
    let err = plan(
        &test_model(),
        &query(json!({ "measures": ["orders.nope"] })),
        &PlanOptions::postgres(),
    )
    .unwrap_err();

    assert!(err.message().contains("nope"), "{err}");
}

#[test]
fn empty_query_is_reported() {
    let err = plan(
        &test_model(),
        &PlannerQuery::default(),
        &PlanOptions::postgres(),
    )
    .unwrap_err();

    assert!(err.message().contains("at least one"), "{err}");
}

#[test]
fn unsupported_member_sql_is_reported_at_load_time_with_the_member_name() {
    let err = Model::from_yaml_str(
        r#"
cubes:
  - name: broken
    sql_table: t
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
      - name: tricky
        type: string
        sql: "{SECURITY_CONTEXT.role.unsafeValue() === 'admin' ? 'a' : 'b'}"
"#,
    )
    .unwrap_err();

    assert!(err.message().contains("broken"), "{err}");
    assert!(err.message().contains("tricky"), "{err}");
}

#[test]
fn javascript_models_are_rejected() {
    let dir = std::env::temp_dir().join("cubeplanner-js-model-test");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("orders.js"), "cube('orders', {});").unwrap();

    let err = Model::from_dir(&dir).unwrap_err();
    std::fs::remove_dir_all(&dir).ok();

    assert!(
        err.message()
            .contains("JavaScript data models are not supported"),
        "{err}"
    );
}

#[test]
fn cubestore_dialect_renders_positional_parameters() {
    let planned = plan_with(
        &test_model(),
        json!({
            "measures": ["orders.count"],
            "filters": [{
                "member": "orders.status",
                "operator": "equals",
                "values": ["shipped"]
            }]
        }),
        &PlanOptions::postgres().with_dialect(Dialect::CubeStore),
    );

    assert!(planned.sql.contains('?'), "{}", planned.sql);
    assert_eq!(planned.param_strings(), vec![Some("shipped".to_string())]);
}
