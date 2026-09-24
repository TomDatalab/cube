//! Unit tests for the member-`sql` parser: Cube's YAML member syntax in,
//! the planner's `CompiledMemberTemplate` out.

use cubeplanner::member_sql::{parse, parse_reference_list, FilterParamsColumn, SqlTemplate};
use serde_json::{json, Value};

fn template(sql: &str) -> String {
    let compiled = parse(sql).unwrap().compile(&Value::Null, None).unwrap();
    match compiled.template {
        SqlTemplate::String(s) => s,
        SqlTemplate::StringVec(items) => items.join(" | "),
    }
}

fn template_with_context(sql: &str, context: Value) -> String {
    let compiled = parse(sql).unwrap().compile(&context, None).unwrap();
    match compiled.template {
        SqlTemplate::String(s) => s,
        SqlTemplate::StringVec(items) => items.join(" | "),
    }
}

#[test]
fn member_references() {
    assert_eq!(template("{CUBE.amount}"), "{arg:0}");
    assert_eq!(template("{orders.amount}"), "{arg:0}");
    assert_eq!(template("{amount}"), "{arg:0}");
    assert_eq!(template("{CUBE}.amount"), "{arg:0}.amount");
    assert_eq!(template("{TABLE}.amount"), "{arg:0}.amount");
    assert_eq!(
        template("SUM({CUBE.amount}) / NULLIF({CUBE.count}, 0)"),
        "SUM({arg:0}) / NULLIF({arg:1}, 0)"
    );
}

#[test]
fn member_reference_paths_are_recorded_once() {
    let parsed = parse("{CUBE.a} + {CUBE.a} + {CUBE.b}").unwrap();
    let compiled = parsed.compile(&Value::Null, None).unwrap();

    assert_eq!(compiled.args.symbol_paths.len(), 2);
    assert_eq!(compiled.args.symbol_paths[0], vec!["CUBE", "a"]);
    assert_eq!(compiled.args.symbol_paths[1], vec!["CUBE", "b"]);
    assert_eq!(parsed.args_names(), &vec!["CUBE".to_string()]);
}

#[test]
fn granularity_reference_keeps_the_whole_path() {
    let compiled = parse("{CUBE.created_at.fiscal_year}")
        .unwrap()
        .compile(&Value::Null, None)
        .unwrap();

    assert_eq!(
        compiled.args.symbol_paths[0],
        vec!["CUBE", "created_at", "fiscal_year"]
    );
}

#[test]
fn literal_braces_and_quoted_sql() {
    assert_eq!(template("{{literal}}"), "{literal}");
    assert_eq!(
        template("CASE WHEN {CUBE.x} THEN '{y}' ELSE 'z' END"),
        "CASE WHEN {arg:0} THEN '{y}' ELSE 'z' END"
    );
}

#[test]
fn filter_params_with_a_string_column() {
    let compiled = parse("SELECT * FROM t WHERE {FILTER_PARAMS.orders.created_at.filter('d')}")
        .unwrap()
        .compile(&Value::Null, None)
        .unwrap();

    assert_eq!(
        compiled.template,
        SqlTemplate::String("SELECT * FROM t WHERE {fp:0}".to_string())
    );
    let item = &compiled.args.filter_params[0];
    assert_eq!(item.cube_name, "orders");
    assert_eq!(item.name, "created_at");
    assert!(item.time_shift_name.is_none());
    assert!(matches!(&item.column, FilterParamsColumn::String(c) if c == "d"));
}

#[test]
fn filter_params_with_a_time_shift() {
    let compiled =
        parse("{FILTER_PARAMS.cal.report_date.time_shifts.prev_fiscal_year.filter('day_date')}")
            .unwrap()
            .compile(&Value::Null, None)
            .unwrap();

    assert_eq!(
        compiled.args.filter_params[0].time_shift_name.as_deref(),
        Some("prev_fiscal_year")
    );
}

#[test]
fn filter_params_with_a_python_lambda_column() {
    let compiled = parse(
        "WHERE {FILTER_PARAMS.events.date.filter(lambda x, y: f\"d >= {x}::date AND d <= {y}::date\")}",
    )
    .unwrap()
    .compile(&Value::Null, None)
    .unwrap();

    match &compiled.args.filter_params[0].column {
        FilterParamsColumn::Compiled(column) => {
            assert_eq!(column.value_params_count, 2);
            assert_eq!(
                column.template,
                SqlTemplate::String("d >= {fpv:0}::date AND d <= {fpv:1}::date".to_string())
            );
        }
        _ => panic!("expected a compiled column"),
    }
}

#[test]
fn filter_params_with_a_triple_quoted_lambda_column() {
    let compiled = parse(
        "{FILTER_PARAMS.events.date.filter(lambda x: f\"\"\"\n  suffix >= FORMAT({x})\n\"\"\")}",
    )
    .unwrap()
    .compile(&Value::Null, None)
    .unwrap();

    match &compiled.args.filter_params[0].column {
        FilterParamsColumn::Compiled(column) => {
            assert_eq!(column.value_params_count, 1);
            assert_eq!(
                column.template,
                SqlTemplate::String("\n  suffix >= FORMAT({fpv:0})\n".to_string())
            );
        }
        _ => panic!("expected a compiled column"),
    }
}

#[test]
fn filter_params_with_a_javascript_arrow_column() {
    let compiled =
        parse("{FILTER_PARAMS.events.date.filter((from, to) => `d >= ${from} AND d <= ${to}`)}")
            .unwrap()
            .compile(&Value::Null, None)
            .unwrap();

    match &compiled.args.filter_params[0].column {
        FilterParamsColumn::Compiled(column) => assert_eq!(
            column.template,
            SqlTemplate::String("d >= {fpv:0} AND d <= {fpv:1}".to_string())
        ),
        _ => panic!("expected a compiled column"),
    }
}

#[test]
fn filter_group_collects_its_bindings() {
    let compiled = parse(
        "WHERE {FILTER_GROUP(\
           FILTER_PARAMS.sales.day.filter('day_d'), \
           FILTER_PARAMS.sales.day.time_shifts.prev_fy.filter(lambda x, y: f\"d >= {x} AND d <= {y}\")\
         )}",
    )
    .unwrap()
    .compile(&Value::Null, None)
    .unwrap();

    assert_eq!(
        compiled.template,
        SqlTemplate::String("WHERE {fg:0}".to_string())
    );
    let group = &compiled.args.filter_groups[0];
    assert_eq!(group.filter_params.len(), 2);
    assert_eq!(
        group.filter_params[1].time_shift_name.as_deref(),
        Some("prev_fy")
    );
    // A grouped binding is recorded in the group only, not also standalone.
    assert!(compiled.args.filter_params.is_empty());
}

#[test]
fn security_context_filter_shapes() {
    let sql = "WHERE {SECURITY_CONTEXT.tenant.filter('tenant_id')}";

    assert_eq!(
        template_with_context(sql, json!({ "tenant": "acme" })),
        "WHERE tenant_id = {sv:0}"
    );
    assert_eq!(
        template_with_context(sql, json!({ "tenant": ["a", "b"] })),
        "WHERE tenant_id IN ({sv:0}, {sv:1})"
    );
    assert_eq!(
        template_with_context(sql, json!({ "tenant": [] })),
        "WHERE 1 = 0"
    );
    assert_eq!(template_with_context(sql, json!({})), "WHERE 1 = 1");
    // Falsy scalars collapse to "no value", as in the JS compiler.
    assert_eq!(
        template_with_context(sql, json!({ "tenant": "" })),
        "WHERE 1 = 1"
    );
}

#[test]
fn security_context_required_filter_is_an_error_without_a_value() {
    let parsed = parse("WHERE {SECURITY_CONTEXT.tenant.requiredFilter('tenant_id')}").unwrap();

    let err = match parsed.compile(&json!({}), None) {
        Err(err) => err,
        Ok(_) => panic!("expected a required-filter error"),
    };
    assert!(
        err.message.contains("Filter for tenant_id is required"),
        "{err}"
    );
}

#[test]
fn security_context_values_are_shared_between_placeholders() {
    let parsed =
        parse("{SECURITY_CONTEXT.t.filter('a')} AND {SECURITY_CONTEXT.t.filter('b')}").unwrap();
    let compiled = parsed.compile(&json!({ "t": "x" }), None).unwrap();

    assert_eq!(
        compiled.template,
        SqlTemplate::String("a = {sv:0} AND b = {sv:0}".to_string())
    );
    assert_eq!(compiled.args.security_context.values, vec!["x".to_string()]);
}

#[test]
fn security_context_unsafe_value_and_nested_paths() {
    assert_eq!(
        template_with_context(
            "{SECURITY_CONTEXT.user.role.unsafeValue()}",
            json!({ "user": { "role": "admin" } })
        ),
        "admin"
    );
    assert_eq!(
        template_with_context(
            "{SECURITY_CONTEXT.user.team.filter('team_id')}",
            json!({ "user": { "team": 7 } })
        ),
        "team_id = {sv:0}"
    );
}

#[test]
fn security_context_snake_case_spelling() {
    assert_eq!(
        template_with_context(
            "{security_context.tenant.filter('t')}",
            json!({ "tenant": "acme" })
        ),
        "t = {sv:0}"
    );
}

#[test]
fn reference_lists_take_paths_and_interpolations() {
    let compiled = parse_reference_list(&[
        "orders.status".to_string(),
        "{CUBE.count}".to_string(),
        "users.city".to_string(),
    ])
    .unwrap()
    .compile(&Value::Null, None)
    .unwrap();

    assert_eq!(
        compiled.template,
        SqlTemplate::StringVec(vec![
            "{arg:0}".to_string(),
            "{arg:1}".to_string(),
            "{arg:2}".to_string(),
        ])
    );
    assert_eq!(compiled.args.symbol_paths[2], vec!["users", "city"]);
}

#[test]
fn unsupported_constructs_are_rejected_with_a_message() {
    // Arbitrary JavaScript.
    let err = parse("{SECURITY_CONTEXT.role.unsafeValue() === 'a' ? 'x' : 'y'}").unwrap_err();
    assert!(err.message().contains("Unsupported"), "{err}");

    // A rest-parameter column callback: no fixed set of placeholders can
    // express it.
    let err = parse("{FILTER_PARAMS.a.b.filter((...values) => `x IN (${values})`)}").unwrap_err();
    assert!(err.message().contains("rest parameter"), "{err}");

    // A malformed FILTER_PARAMS path.
    let err = parse("{FILTER_PARAMS.a.filter('x')}").unwrap_err();
    assert!(err.message().contains("FILTER_PARAMS"), "{err}");

    // An unterminated interpolation.
    let err = parse("{CUBE.amount").unwrap_err();
    assert!(err.message().contains("Unclosed brace"), "{err}");

    // A SQL_UTILS method this compiler has no Rust equivalent for.
    let parsed = parse("{SQL_UTILS.somethingElse('x')}").unwrap();
    let err = match parsed.compile(&Value::Null, None) {
        Err(err) => err,
        Ok(_) => panic!("expected an unsupported-method error"),
    };
    assert!(err.message.contains("not supported"), "{err}");
}
