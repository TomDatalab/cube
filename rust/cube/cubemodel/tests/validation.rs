//! Validation messages, matching `CubeValidator` / `ErrorReporter` wording.

use cubemodel::ModelLoader;

fn errors(source: &str) -> Vec<String> {
    let err = ModelLoader::load_str(source, "model.yml").unwrap_err();
    err.items().iter().map(|i| i.message.clone()).collect()
}

fn assert_contains(messages: &[String], needle: &str) {
    assert!(
        messages.iter().any(|m| m.contains(needle)),
        "expected a message containing {needle:?}, got {messages:#?}"
    );
}

#[test]
fn requires_sql_or_sql_table_but_not_both() {
    assert_contains(
        &errors(
            "cubes:\n  - name: orders\n    measures:\n      - name: count\n        type: count\n",
        ),
        "You must use either sql or sqlTable within a model, but not both",
    );
    assert_contains(
        &errors(
            "cubes:\n  - name: orders\n    sql: SELECT 1\n    sql_table: public.orders\n    measures:\n      - name: count\n        type: count\n",
        ),
        "You must use either sql or sqlTable within a model, but not both",
    );
}

#[test]
fn reports_unknown_keys() {
    let messages = errors(
        r#"
cubes:
  - name: orders
    sql: SELECT 1
    not_a_real_key: oops
    measures:
      - name: count
        type: count
        another_bad_key: 42
"#,
    );
    assert_contains(&messages, "(notARealKey = oops) is not allowed");
    assert_contains(
        &messages,
        "(measures.count.anotherBadKey = 42) is not allowed",
    );
}

#[test]
fn validates_measure_types_and_required_sql() {
    let messages = errors(
        r#"
cubes:
  - name: orders
    sql: SELECT 1
    measures:
      - name: bad
        sql: amount
        type: totally_wrong # camelized to totallyWrong, like camelCaseTypes
      - name: no_sql
        type: sum
      - name: no_type
        sql: amount
"#,
    );
    assert_contains(
        &messages,
        "(measures.bad.type = totallyWrong) must be one of [count, number, string, boolean, time, sum, avg, min, max, countDistinct, countDistinctApprox]",
    );
    assert_contains(&messages, "(measures.no_sql.sql) is required");
    assert_contains(&messages, "(measures.no_type.type) is required");
}

#[test]
fn validates_dimension_types_and_required_sql() {
    let messages = errors(
        r#"
cubes:
  - name: orders
    sql: SELECT 1
    dimensions:
      - name: bad
        sql: x
        type: nope
      - name: no_sql
        type: string
      - name: not_a_time
        sql: x
        type: string
        granularities:
          - name: fiscal_year
            interval: 1 year
"#,
    );
    assert_contains(
        &messages,
        "(dimensions.bad.type = nope) must be one of [string, number, boolean, time, geo]",
    );
    assert_contains(&messages, "(dimensions.no_sql.sql) is required");
    assert_contains(
        &messages,
        "(dimensions.not_a_time.granularities) is not allowed",
    );
}

#[test]
fn granularity_with_sql_must_use_a_predefined_name() {
    let messages = errors(
        r#"
cubes:
  - name: orders
    sql: SELECT 1
    dimensions:
      - name: created_at
        sql: created_at
        type: time
        granularities:
          - name: fiscal_year
            sql: "date_trunc('year', created_at)"
"#,
    );
    assert_contains(
        &messages,
        "dimensions.created_at.granularities.fiscal_year: a granularity defined with 'sql' must be named after one of the predefined granularities (second, minute, hour, day, week, month, quarter, year). Define 'fiscal_year' with 'interval' instead",
    );
}

#[test]
fn validates_join_relationship_and_target() {
    let messages = errors(
        r#"
cubes:
  - name: orders
    sql: SELECT 1
    joins:
      - name: users
        sql: "{CUBE}.user_id = {users}.id"
        relationship: sideways
      - name: ghosts
        sql: "1 = 1"
        relationship: many_to_one
    measures:
      - name: count
        type: count
"#,
    );
    assert_contains(
        &messages,
        "(joins[0].relationship = sideways) must be one of [",
    );
    assert_contains(&messages, "Cube ghosts doesn't exist");
}

#[test]
fn reports_duplicate_member_names() {
    let messages = errors(
        r#"
cubes:
  - name: orders
    sql: SELECT 1
    measures:
      - name: count
        type: count
      - name: count
        type: count
    dimensions:
      - name: status
        sql: status
        type: string
    segments:
      - name: status
        sql: "1 = 1"
"#,
    );
    assert_contains(
        &messages,
        "Member names must be unique within a cube. Found duplicate measure 'count' in cube 'orders'.",
    );
    assert_contains(&messages, "status defined more than once");
}

#[test]
fn validates_pre_aggregation_rules() {
    let messages = errors(
        r#"
cubes:
  - name: orders
    sql: SELECT 1
    measures:
      - name: count
        type: count
    dimensions:
      - name: created_at
        sql: created_at
        type: time
    pre_aggregations:
      - name: no_granularity
        type: rollup
        measures:
          - count
        time_dimension: created_at
      - name: bad_type
        type: rollupy
      - name: lambda_without_rollups
        type: rollupLambda
      - name: bad_partition
        type: rollup
        measures:
          - count
        partition_granularity: fortnight
"#,
    );
    assert_contains(
        &messages,
        "(preAggregations.no_granularity.granularity) is required",
    );
    assert_contains(
        &messages,
        "(preAggregations.bad_type.type = rollupy) must be one of [autoRollup, originalSql, rollup, rollupJoin, rollupLambda]",
    );
    assert_contains(
        &messages,
        "(preAggregations.lambda_without_rollups.rollups) is required",
    );
    assert_contains(
        &messages,
        "(preAggregations.bad_partition.partitionGranularity = fortnight) must be one of [hour, day, week, month, quarter, year]",
    );
}

#[test]
fn view_includes_must_reference_known_members() {
    let messages = errors(
        r#"
cubes:
  - name: orders
    sql: SELECT 1
    measures:
      - name: count
        type: count
    dimensions:
      - name: status
        sql: status
        type: string
views:
  - name: orders_view
    cubes:
      - join_path: orders
        includes:
          - count
          - not_a_member
"#,
    );
    assert_contains(
        &messages,
        "Member 'not_a_member' is included in 'orders_view' but not defined in any cube",
    );
}

#[test]
fn view_includes_reject_paths() {
    let messages = errors(
        r#"
cubes:
  - name: orders
    sql: SELECT 1
    measures:
      - name: count
        type: count
views:
  - name: orders_view
    cubes:
      - join_path: orders
        includes:
          - orders.count
"#,
    );
    assert_contains(
        &messages,
        "Paths aren't allowed in cube includes but 'orders.count' provided as include member",
    );
}

#[test]
fn view_cannot_reach_a_cube_by_two_join_paths() {
    let messages = errors(
        r#"
cubes:
  - name: orders
    sql: SELECT 1
    measures:
      - name: count
        type: count
  - name: users
    sql: SELECT 1
    dimensions:
      - name: city
        sql: city
        type: string
  - name: companies
    sql: SELECT 1
    dimensions:
      - name: name
        sql: name
        type: string
views:
  - name: orders_view
    cubes:
      - join_path: orders.users
        includes: "*"
      - join_path: orders.companies.users
        includes: "*"
"#,
    );
    assert_contains(
        &messages,
        "Views can't define multiple join paths to the same cube. View 'orders_view' has multiple paths to 'users' within root 'orders': 'orders.users', 'orders.companies.users'. Use extends to create a child cube and reference it instead",
    );
}

#[test]
fn reports_folder_problems() {
    let messages = errors(
        r#"
cubes:
  - name: orders
    sql: SELECT 1
    measures:
      - name: count
        type: count
    dimensions:
      - name: status
        sql: status
        type: string
views:
  - name: orders_view
    cubes:
      - join_path: orders
        includes: "*"
    folders:
      - name: folder1
        includes:
          - orders.status
      - name: folder1
        includes:
          - count
"#,
    );
    assert_contains(
        &messages,
        "Paths aren't allowed in the 'folders' but 'orders.status' has been provided for orders_view",
    );
    assert_contains(
        &messages,
        "Member 'orders.status' included in folder 'folder1' not found",
    );
    assert_contains(
        &messages,
        "Folder names must be unique within a view. Found duplicate folder 'folder1' in view 'orders_view'.",
    );
}

#[test]
fn split_and_prefix_are_mutually_exclusive() {
    let messages = errors(
        r#"
cubes:
  - name: orders
    sql: SELECT 1
    measures:
      - name: count
        type: count
views:
  - name: orders_view
    cubes:
      - join_path: orders
        prefix: true
        split: true
        includes: "*"
"#,
    );
    assert_contains(
        &messages,
        "Using split together with prefix is not supported",
    );
}

#[test]
fn collects_every_error_not_just_the_first() {
    let messages = errors(
        r#"
cubes:
  - name: a
    measures:
      - name: bad
        type: whoops
  - name: b
    measures:
      - name: also_bad
        type: whoops2
"#,
    );
    assert!(messages.len() >= 4, "got {messages:#?}");
    let rendered = ModelLoader::load_str(
        "cubes:\n  - name: a\n    measures:\n      - name: bad\n        type: whoops\n",
        "a.yml",
    )
    .unwrap_err()
    .to_string();
    assert!(rendered.starts_with("a.yml Errors:"), "{rendered}");
}
