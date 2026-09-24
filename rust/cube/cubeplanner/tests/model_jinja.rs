//! A Jinja data model must plan, not only compile for `/v1/meta`.
//!
//! `cubemodel` renders Jinja before parsing YAML; the planner used to parse
//! the raw file, so a model that served `/v1/meta` failed every query. Both
//! now render through the same engine.

use cubemodel::TemplateContext;
use cubeplanner::{plan, Model, PlanOptions, PlannerQuery};
use serde_json::json;

const JINJA_MODEL: &str = r#"
cubes:
  - name: orders
    sql_table: public.orders
    measures:
      {%- for m in ['count'] %}
      - name: {{ m }}
        type: {{ m }}
      {%- endfor %}
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
      {%- for d in ['status'] %}
      - name: {{ d }}
        sql: {{ d }}
        type: string
      {%- endfor %}
"#;

#[test]
fn a_jinja_model_loads_and_plans() {
    let model = Model::from_yaml_str(JINJA_MODEL).expect("the model renders and parses");
    assert_eq!(model.cube_names(), vec!["orders"]);

    let query = PlannerQuery::from_value(
        json!({ "measures": ["orders.count"], "dimensions": ["orders.status"] }),
    )
    .expect("valid query");
    let planned = plan(&model, &query, &PlanOptions::postgres()).expect("plans");

    assert!(planned.sql.contains("public.orders"), "{}", planned.sql);
    assert!(planned.sql.contains("count("), "{}", planned.sql);
}

#[test]
fn a_model_with_no_jinja_is_untouched() {
    let plain = r#"
cubes:
  - name: orders
    sql_table: public.orders
    measures:
      - name: count
        type: count
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
"#;
    let model = Model::from_yaml_str(plain).expect("parses");
    assert_eq!(model.cube_names(), vec!["orders"]);
}

#[test]
fn the_compile_context_reaches_the_planner() {
    // A multi-tenant model branches on `COMPILE_CONTEXT`, so the planner has
    // to render with the same context `/v1/meta` used.
    let model_source = r#"
cubes:
  - name: orders
    sql_table: {{ COMPILE_CONTEXT.table }}
    measures:
      - name: count
        type: count
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
"#;

    let context = TemplateContext {
        compile_context: json!({ "table": "tenant_a.orders" }),
        variables: json!({}),
    };
    let model = Model::from_yaml_documents_with_context([("orders.yml", model_source)], &context)
        .expect("renders with the context");

    let query = PlannerQuery::from_value(json!({ "measures": ["orders.count"] })).unwrap();
    let planned = plan(&model, &query, &PlanOptions::postgres()).expect("plans");
    assert!(planned.sql.contains("tenant_a.orders"), "{}", planned.sql);

    // A different tenant reads a different table from the same file.
    let context = TemplateContext {
        compile_context: json!({ "table": "tenant_b.orders" }),
        variables: json!({}),
    };
    let model = Model::from_yaml_documents_with_context([("orders.yml", model_source)], &context)
        .expect("renders with the context");
    let planned = plan(&model, &query, &PlanOptions::postgres()).expect("plans");
    assert!(planned.sql.contains("tenant_b.orders"), "{}", planned.sql);
}

#[test]
fn a_broken_template_is_reported_with_the_file_name() {
    let broken = "cubes:\n  {% for x in %}\n";
    let err = Model::from_yaml_documents([("orders.yml", broken)])
        .expect_err("the template does not render");
    assert!(err.message().contains("orders.yml"), "{}", err.message());
}
