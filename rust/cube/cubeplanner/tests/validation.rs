//! The loader is strict and eager: an unknown key is a typo, not something to
//! drop, and every `sql:` in the model is parsed at load time, named by the
//! cube and member it sits on.

use cubeplanner::Model;

fn load_error(yaml: &str) -> String {
    Model::from_yaml_str(yaml)
        .err()
        .map(|err| err.message())
        .unwrap_or_else(|| panic!("expected the model to be rejected:\n{yaml}"))
}

fn loads(yaml: &str) {
    Model::from_yaml_str(yaml).unwrap_or_else(|e| panic!("{e}\n{yaml}"));
}

/// A cube with everything this planner supports, used as the base every case
/// below mutates.
const VALID: &str = r#"
cubes:
  - name: orders
    sql_table: public.orders
    joins:
      - name: users
        relationship: many_to_one
        sql: "{CUBE}.user_id = {users}.id"
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
      - name: user_id
        sql: user_id
        type: number
      - name: status
        sql: status
        type: string
      - name: created_at
        sql: created_at
        type: time
        granularities:
          - name: half_year
            interval: 6 months
    measures:
      - name: count
        type: count
        sql: id
    segments:
      - name: completed
        sql: "{CUBE}.status = 'completed'"
    pre_aggregations:
      - name: main
        type: rollup
        measures:
          - count
        dimensions:
          - status
        time_dimension: created_at
        granularity: day
  - name: users
    sql_table: public.users
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
"#;

#[test]
fn the_reference_model_loads() {
    loads(VALID);
}

// ---- unknown keys ----------------------------------------------------------

#[test]
fn an_unknown_cube_key_is_reported_with_the_cube_and_the_key() {
    let message =
        load_error(&VALID.replace("    sql_table: public.orders", "    sqlTable: orders"));

    assert!(message.contains("Cube `orders`"), "{message}");
    assert!(message.contains("unknown cube key `sqlTable`"), "{message}");
    assert!(message.contains("sql_table"), "{message}");
}

#[test]
fn an_unknown_dimension_key_is_reported_with_the_member() {
    let message = load_error(&VALID.replace(
        "      - name: status\n        sql: status\n        type: string",
        "      - name: status\n        sql: status\n        type: string\n        primaryKey: true",
    ));

    assert!(message.contains("Cube `orders`"), "{message}");
    assert!(message.contains("dimension `status`"), "{message}");
    assert!(
        message.contains("unknown dimension key `primaryKey`"),
        "{message}"
    );
}

#[test]
fn an_unknown_measure_key_is_reported_with_the_member() {
    let message = load_error(&VALID.replace(
        "      - name: count\n        type: count\n        sql: id",
        "      - name: count\n        type: count\n        sql: id\n        rollingWindow:\n          trailing: 7 day",
    ));

    assert!(message.contains("measure `count`"), "{message}");
    assert!(
        message.contains("unknown measure key `rollingWindow`"),
        "{message}"
    );
}

#[test]
fn an_unknown_pre_aggregation_key_is_reported() {
    let message = load_error(&VALID.replace(
        "        granularity: day",
        "        granularity: day\n        partitionGranularity: month",
    ));

    assert!(message.contains("pre-aggregation `main`"), "{message}");
    assert!(
        message.contains("unknown pre-aggregation key `partitionGranularity`"),
        "{message}"
    );
}

#[test]
fn an_unknown_view_key_is_reported() {
    let message = load_error(
        r#"
cubes:
  - name: orders
    sql_table: public.orders
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
views:
  - name: sales
    cubes:
      - joinPath: orders
        includes: "*"
"#,
    );

    assert!(message.contains("View `sales`"), "{message}");
    assert!(
        message.contains("unknown view cube key `joinPath`"),
        "{message}"
    );
}

/// Presentation-only keys are documented Cube YAML that never reaches the SQL,
/// so they are accepted rather than treated as typos.
#[test]
fn presentation_only_keys_are_accepted() {
    loads(&VALID.replace(
        "      - name: status\n        sql: status\n        type: string",
        "      - name: status\n        sql: status\n        type: string\n        title: Status\n        description: The order status\n        public: false",
    ));
}

// ---- eager sql validation --------------------------------------------------

#[test]
fn a_join_sql_the_parser_cannot_read_is_reported_at_load_time() {
    let message = load_error(&VALID.replace(
        r#"        sql: "{CUBE}.user_id = {users}.id""#,
        r#"        sql: "{CUBE}.user_id = {SECURITY_CONTEXT.x.unsafeValue() ? 1 : 2}""#,
    ));

    assert!(message.contains("Cube `orders`"), "{message}");
    assert!(message.contains("join `users`"), "{message}");
}

#[test]
fn a_join_without_sql_is_reported() {
    let message = load_error(&VALID.replace(r#"        sql: "{CUBE}.user_id = {users}.id""#, ""));

    assert!(message.contains("join `users`"), "{message}");
    assert!(message.contains("`sql:` is required"), "{message}");
}

#[test]
fn a_measure_filter_sql_is_parsed_at_load_time() {
    let message = load_error(&VALID.replace(
        "      - name: count\n        type: count\n        sql: id",
        "      - name: count\n        type: count\n        sql: id\n        filters:\n          - sql: \"{SECURITY_CONTEXT.role.unsafeValue() === 'a'}\"",
    ));

    assert!(message.contains("measure `count`"), "{message}");
    assert!(message.contains("filters[0]"), "{message}");
}

#[test]
fn a_measure_filter_without_sql_is_reported() {
    let message = load_error(&VALID.replace(
        "      - name: count\n        type: count\n        sql: id",
        "      - name: count\n        type: count\n        sql: id\n        filters:\n          - dir: asc",
    ));

    assert!(message.contains("measure `count`"), "{message}");
    assert!(message.contains("`sql:` is required"), "{message}");
}

#[test]
fn a_measure_order_by_sql_is_parsed_at_load_time() {
    let message = load_error(&VALID.replace(
        "      - name: count\n        type: count\n        sql: id",
        "      - name: count\n        type: count\n        sql: id\n        order_by:\n          - sql: \"{SECURITY_CONTEXT.role.unsafeValue() === 'a'}\"\n            dir: asc",
    ));

    assert!(message.contains("measure `count`"), "{message}");
    assert!(message.contains("order_by[0]"), "{message}");
}

#[test]
fn a_case_branch_sql_is_parsed_at_load_time() {
    let message = load_error(&VALID.replace(
        "      - name: status\n        sql: status\n        type: string",
        "      - name: status\n        type: string\n        case:\n          when:\n            - sql: \"{SECURITY_CONTEXT.role.unsafeValue() === 'a'}\"\n              label: A\n          else:\n            label: B",
    ));

    assert!(message.contains("dimension `status`"), "{message}");
    assert!(message.contains("case.when[0]"), "{message}");
}

#[test]
fn a_case_without_an_else_is_reported() {
    let message = load_error(&VALID.replace(
        "      - name: status\n        sql: status\n        type: string",
        "      - name: status\n        type: string\n        case:\n          when:\n            - sql: \"{CUBE}.status = 'a'\"\n              label: A",
    ));

    assert!(message.contains("dimension `status`"), "{message}");
    assert!(message.contains("`else:`"), "{message}");
}

#[test]
fn a_case_switch_branch_sql_is_parsed_at_load_time() {
    let message = load_error(&VALID.replace(
        "      - name: status\n        sql: status\n        type: string",
        "      - name: status\n        type: string\n        case:\n          switch: \"{CUBE}.status\"\n          when:\n            - value: a\n              sql: \"{SECURITY_CONTEXT.role.unsafeValue() === 'a'}\"\n          else:\n            sql: \"'other'\"",
    ));

    assert!(message.contains("dimension `status`"), "{message}");
    assert!(message.contains("case.when[0]"), "{message}");
}

#[test]
fn a_granularity_sql_is_parsed_at_load_time() {
    let message = load_error(&VALID.replace(
        "          - name: half_year\n            interval: 6 months",
        "          - name: half_year\n            interval: 6 months\n            sql: \"{SECURITY_CONTEXT.role.unsafeValue() === 'a'}\"",
    ));

    assert!(message.contains("dimension `created_at`"), "{message}");
    assert!(message.contains("granularity `half_year`"), "{message}");
}

#[test]
fn a_custom_granularity_without_an_interval_is_reported() {
    let message = load_error(&VALID.replace(
        "          - name: half_year\n            interval: 6 months",
        "          - name: half_year",
    ));

    assert!(message.contains("granularity `half_year`"), "{message}");
    assert!(message.contains("`interval:`"), "{message}");
}

/// A pre-aggregation reference list used to abort the process on an
/// unparseable entry, because the loader `expect`s it.
#[test]
fn a_pre_aggregation_reference_list_is_parsed_at_load_time() {
    let message = load_error(&VALID.replace(
        "        dimensions:\n          - status",
        "        dimensions:\n          - \"{SECURITY_CONTEXT.role.unsafeValue() === 'a'}\"",
    ));

    assert!(message.contains("pre-aggregation `main`"), "{message}");
    assert!(message.contains("dimensions"), "{message}");
}

#[test]
fn a_pre_aggregation_time_dimension_is_parsed_at_load_time() {
    let message = load_error(&VALID.replace(
        "        time_dimension: created_at",
        "        time_dimension: \"{SECURITY_CONTEXT.role.unsafeValue() === 'a'}\"",
    ));

    assert!(message.contains("pre-aggregation `main`"), "{message}");
    assert!(message.contains("time_dimension"), "{message}");
}

#[test]
fn a_mask_sql_is_parsed_at_load_time() {
    let message = load_error(&VALID.replace(
        "      - name: status\n        sql: status\n        type: string",
        "      - name: status\n        sql: status\n        type: string\n        mask:\n          sql: \"{SECURITY_CONTEXT.role.unsafeValue() === 'a'}\"",
    ));

    assert!(message.contains("dimension `status`"), "{message}");
    assert!(message.contains("mask"), "{message}");
}

#[test]
fn a_member_without_a_type_is_reported() {
    let message = load_error(&VALID.replace(
        "      - name: status\n        sql: status\n        type: string",
        "      - name: status\n        sql: status",
    ));

    assert!(message.contains("dimension `status`"), "{message}");
    assert!(message.contains("`type:` is required"), "{message}");
}

#[test]
fn a_member_without_a_name_is_reported() {
    let message = load_error(&VALID.replace(
        "      - name: status\n        sql: status\n        type: string",
        "      - sql: status\n        type: string",
    ));

    assert!(message.contains("needs a `name:`"), "{message}");
}

/// A view that declares members of its own is checked the same way.
#[test]
fn a_view_member_is_checked_too() {
    let message = load_error(
        r#"
cubes:
  - name: orders
    sql_table: public.orders
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
views:
  - name: sales
    dimensions:
      - name: broken
        type: string
        sql: "{SECURITY_CONTEXT.role.unsafeValue() === 'a'}"
"#,
    );

    assert!(message.contains("View `sales`"), "{message}");
    assert!(message.contains("dimension `broken`"), "{message}");
}
