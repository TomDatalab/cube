//! Loading, normalisation, Jinja and `extends`.

use cubemodel::model::Includes;
use cubemodel::{ModelError, ModelLoader};

const SIMPLE: &str = r#"
cubes:
  - name: orders
    sql_table: public.orders
    data_source: warehouse
    meta:
      owner_team: sales
      nested_key_here: keep_me
    measures:
      - name: count
        type: count
      - name: total
        sql: "{CUBE}.amount"
        type: sum
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
      - name: created_at
        sql: created_at
        type: time
        granularities:
          - name: fiscal_year_2024
            interval: 1 year
            origin: "2024-02-01"
    segments:
      - name: big
        sql: "{CUBE}.amount > 100"
    pre_aggregations:
      - name: main
        type: rollup
        measures:
          - count
        time_dimension: created_at
        granularity: day
"#;

#[test]
fn loads_a_cube_with_all_member_kinds() {
    let model = ModelLoader::load_str(SIMPLE, "orders.yml").unwrap();

    assert_eq!(model.cubes.len(), 1);
    assert!(model.views.is_empty());

    let orders = &model.cubes["orders"];
    assert_eq!(orders.file_name, "orders.yml");
    assert!(!orders.is_view);
    // snake_case keys are camelized, raw SQL survives verbatim.
    assert_eq!(orders.sql_table.as_deref(), Some("public.orders"));
    assert_eq!(orders.data_source.as_deref(), Some("warehouse"));
    assert_eq!(
        orders.measures["total"].sql.as_deref(),
        Some("{CUBE}.amount")
    );
    assert_eq!(
        orders.segments["big"].sql.as_deref(),
        Some("{CUBE}.amount > 100")
    );

    // `primary_key` -> `primaryKey`
    assert_eq!(orders.dimensions["id"].primary_key, Some(true));

    // Granularity names keep their original casing (IGNORE_CAMELIZE).
    let created_at = &orders.dimensions["created_at"];
    assert!(created_at.granularities.contains_key("fiscal_year_2024"));
    assert_eq!(
        created_at.granularities["fiscal_year_2024"]
            .interval
            .as_deref(),
        Some("1 year")
    );

    // `meta` is user data: its keys are never camelized.
    let meta = orders.meta.as_ref().unwrap();
    assert!(meta.get("owner_team").is_some());
    assert!(meta.get("nested_key_here").is_some());
    assert!(meta.get("ownerTeam").is_none());

    let pre_agg = &orders.pre_aggregations["main"];
    assert_eq!(pre_agg.pre_agg_type.as_deref(), Some("rollup"));
    assert_eq!(pre_agg.time_dimension.as_deref(), Some("created_at"));
    assert_eq!(pre_agg.granularity.as_deref(), Some("day"));
}

#[test]
fn loads_views_with_cube_includes() {
    let source = format!(
        "{SIMPLE}
views:
  - name: orders_view
    cubes:
      - join_path: orders
        includes: \"*\"
        excludes:
          - id
"
    );
    let model = ModelLoader::load_str(&source, "model.yml").unwrap();
    let view = &model.views["orders_view"];
    assert!(view.is_view);
    // `join_path` -> `joinPath`
    assert_eq!(view.cubes[0].join_path.as_deref(), Some("orders"));
    assert!(matches!(view.cubes[0].includes, Includes::All(_)));
    assert_eq!(view.cubes[0].excludes, vec!["id".to_string()]);
}

#[test]
fn rejects_unexpected_top_level_keys() {
    let err = ModelLoader::load_str("cube:\n  - name: orders\n", "bad.yml").unwrap_err();
    assert!(err.messages().iter().any(|m| {
        m == "Unexpected YAML key: cube. Only 'cubes', 'views', and 'view_groups' are allowed here."
    }));
}

#[test]
fn reports_duplicate_cube_names() {
    let source = r#"
cubes:
  - name: orders
    sql: SELECT 1
    measures:
      - name: count
        type: count
  - name: orders
    sql: SELECT 2
    measures:
      - name: count
        type: count
"#;
    let err = ModelLoader::load_str(source, "dup.yml").unwrap_err();
    assert!(err
        .messages()
        .iter()
        .any(|m| m == "Found duplicate cube name 'orders'."));
}

#[test]
fn errors_carry_cube_name_and_file() {
    let err = ModelLoader::load_str(
        "cubes:\n  - name: orders\n    measures:\n      - name: count\n        type: count\n",
        "cubes/orders.yml",
    )
    .unwrap_err();
    let item = err
        .items()
        .iter()
        .find(|i| i.message.starts_with("You must use either sql"))
        .expect("sql/sqlTable error");
    assert_eq!(item.context, vec!["orders cube".to_string()]);
    assert_eq!(item.file_name.as_deref(), Some("cubes/orders.yml"));
    assert_eq!(
        item.full_message(),
        "orders cube: You must use either sql or sqlTable within a model, but not both"
    );
}

#[test]
fn extends_merges_members_and_hides_sql() {
    let source = r#"
cubes:
  - name: base_orders
    sql: SELECT * FROM orders
    measures:
      - name: count
        type: count
    dimensions:
      - name: id
        sql: id
        type: number
        primary_key: true
  - name: child_orders
    extends: base_orders
    sql_table: public.orders
    dimensions:
      - name: status
        sql: status
        type: string
"#;
    let model = ModelLoader::load_str(source, "orders.yml").unwrap();
    let child = &model.cubes["child_orders"];
    assert!(child.measures.contains_key("count"));
    assert!(child.dimensions.contains_key("id"));
    assert!(child.dimensions.contains_key("status"));
    // `sql_table` in the child hides the parent's `sql`.
    assert_eq!(child.sql_table.as_deref(), Some("public.orders"));
    assert!(child.sql.is_none());
}

#[test]
fn renders_jinja_with_env_var() {
    std::env::set_var("CUBEMODEL_TEST_TABLE", "public.orders");
    let source = r#"
cubes:
  - name: orders
    sql_table: {{ env_var('CUBEMODEL_TEST_TABLE') }}
    measures:
      - name: count
        type: count
"#;
    let model = ModelLoader::load_str(source, "orders.yml").unwrap();
    // Templates auto-escape as JSON, matching the Node jinja engine setup.
    assert_eq!(
        model.cubes["orders"].sql_table.as_deref(),
        Some("public.orders")
    );
}

#[test]
fn renders_jinja_loops() {
    let source = r#"
cubes:
  - name: orders
    sql: SELECT * FROM orders
    dimensions:
      {% for name in ['status', 'city'] %}
      - name: {{ name }}
        sql: {{ name }}
        type: string
      {% endfor %}
"#;
    let model = ModelLoader::load_str(source, "orders.yml").unwrap();
    let dims = &model.cubes["orders"].dimensions;
    assert!(dims.contains_key("status"));
    assert!(dims.contains_key("city"));
}

#[test]
fn unknown_jinja_function_fails_loudly() {
    let source = r#"
cubes:
  - name: orders
    sql: SELECT * FROM {{ my_python_function('orders') }}
    measures:
      - name: count
        type: count
"#;
    let err = ModelLoader::load_str(source, "orders.yml").unwrap_err();
    let ModelError::Template { file_name, message } = &err else {
        panic!("expected a template error, got {err:?}");
    };
    assert_eq!(file_name, "orders.yml");
    assert!(message.contains("my_python_function"));
    assert!(message.contains("cube.py"));
}

#[test]
fn rejects_javascript_model_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("orders.yml"),
        "cubes:\n  - name: orders\n    sql: SELECT 1\n    measures:\n      - name: count\n        type: count\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("legacy.js"), "cube('Legacy', {});").unwrap();

    let err = ModelLoader::load_dir(dir.path()).unwrap_err();
    let ModelError::UnsupportedModelFile { file_name, .. } = &err else {
        panic!("expected an unsupported-file error, got {err:?}");
    };
    assert_eq!(file_name, "legacy.js");
    assert_eq!(
        err.to_string(),
        "JavaScript data models are not supported: legacy.js. Convert it to YAML."
    );
}

#[test]
fn rejects_python_model_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("cube.py"), "from cube import template\n").unwrap();

    let err = ModelLoader::load_dir(dir.path()).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Python data models and template functions are not supported: cube.py. Convert it to YAML."
    );
}

#[test]
fn load_dir_reads_nested_directories() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("cubes")).unwrap();
    std::fs::create_dir_all(dir.path().join("views")).unwrap();
    std::fs::write(
        dir.path().join("cubes/orders.yml"),
        "cubes:\n  - name: orders\n    sql: SELECT 1\n    measures:\n      - name: count\n        type: count\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("views/orders_view.yaml"),
        "views:\n  - name: orders_view\n    cubes:\n      - join_path: orders\n        includes: \"*\"\n",
    )
    .unwrap();

    let model = ModelLoader::load_dir(dir.path()).unwrap();
    assert_eq!(model.cubes.len(), 1);
    assert_eq!(model.views.len(), 1);
    assert_eq!(model.cubes["orders"].file_name, "cubes/orders.yml");
}

#[test]
fn missing_directory_is_an_io_error() {
    let err = ModelLoader::load_dir("tests/fixtures/does-not-exist").unwrap_err();
    assert!(matches!(err, ModelError::Io(_)));
}
