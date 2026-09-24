//! Coverage over the schema-compiler's own YAML fixtures, copied verbatim into
//! `tests/fixtures/`.

use cubemodel::{meta_config, ModelLoader};

fn source(fixture: &str) -> String {
    std::fs::read_to_string(format!("tests/fixtures/{fixture}")).unwrap()
}

#[test]
fn format_showcase_parses_and_resolves_formats() {
    let model = ModelLoader::load_str(&source("format_showcase.yml"), "format_showcase.yml")
        .unwrap_or_else(|e| panic!("{}", e.messages().join("\n")));

    let config = meta_config(&model);
    let cube = config
        .cubes
        .iter()
        .find(|c| c.name == "format_showcase")
        .unwrap();

    let measure = |name: &str| {
        cube.config["measures"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["name"] == format!("format_showcase.{name}"))
            .unwrap_or_else(|| panic!("no measure {name}"))
            .clone()
    };
    let dimension = |name: &str| {
        cube.config["dimensions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == format!("format_showcase.{name}"))
            .unwrap_or_else(|| panic!("no dimension {name}"))
            .clone()
    };

    let usd = measure("revenue_usd");
    assert_eq!(usd["format"], "currency");
    assert_eq!(usd["currency"], "USD");
    assert_eq!(usd["formatDescription"]["specifier"], "$,.2~f");

    // `format: link` has no named-numeric equivalent, so it stays a bare string.
    assert_eq!(dimension("url_link")["format"], "link");
    // `format: id` resolves through NAMED_NUMERIC_FORMATS first, like Node.
    assert_eq!(dimension("product_id")["format"]["type"], "custom-numeric");
    assert_eq!(dimension("product_id")["format"]["value"], ".0f");
    assert_eq!(dimension("product_id")["format"]["alias"], "id");
    // Non-number dimensions get no formatDescription.
    assert!(dimension("url_link").get("formatDescription").is_none());
}

#[test]
fn switch_dimension_is_exposed_as_a_string() {
    let model = ModelLoader::load_str(&source("switch-dimension.yml"), "switch-dimension.yml")
        .unwrap_or_else(|e| panic!("{}", e.messages().join("\n")));
    let config = meta_config(&model);
    let switch_dimensions: Vec<_> = config
        .cubes
        .iter()
        .flat_map(|c| c.config["dimensions"].as_array().unwrap().clone())
        .filter(|d| d["type"] == "string")
        .collect();
    assert!(!switch_dimensions.is_empty());
}

#[test]
fn hierarchy_with_a_measure_is_rejected() {
    let err = ModelLoader::load_str(
        &source("hierarchy-with-measure.yml"),
        "hierarchy-with-measure.yml",
    )
    .unwrap_err();
    assert!(err.messages().iter().any(|m| m.contains(
        "Only dimensions can be part of a hierarchy. Please remove the 'count' member from the 'orders_hierarchy' hierarchy."
    )), "{:#?}", err.messages());
}

#[test]
fn folder_with_a_missing_member_is_rejected() {
    let err = ModelLoader::load_str(&source("folders_non_exist.yml"), "folders_non_exist.yml")
        .unwrap_err();
    assert!(err
        .messages()
        .iter()
        .any(|m| m.contains("Member 'non-existent' included in folder 'folder1' not found")));
}

#[test]
fn validate_preaggs_fixture_reports_every_failure() {
    let err =
        ModelLoader::load_str(&source("validate_preaggs.yml"), "validate_preaggs.yml").unwrap_err();
    let messages = err.messages();
    for expected in [
        "(preAggregations.autoRollupFail.maxPreAggregations = string_instead_of_number) must be a number",
        "(preAggregations.originalSqlFail.timeDimension) is required",
        "(preAggregations.originalSqlFail.partitionGranularity = invalid_partition_granularity) must be one of [hour, day, week, month, quarter, year]",
        "(preAggregations.rollupJoinFail.rollups) is required",
    ] {
        assert!(
            messages.iter().any(|m| m.contains(expected)),
            "missing {expected:?} in {messages:#?}"
        );
    }
}

#[test]
fn hierarchies_and_folders_fixtures_compile_cleanly() {
    for fixture in ["hierarchies.yml", "folders.yml"] {
        ModelLoader::load_str(&source(fixture), fixture)
            .unwrap_or_else(|e| panic!("{fixture}: {}", e.messages().join("\n")));
    }
}

#[test]
fn snake_case_types_are_camelized() {
    let model = ModelLoader::load_str(
        r#"
cubes:
  - name: orders
    sql: SELECT 1
    joins:
      - name: users
        sql: "1 = 1"
        relationship: many_to_one
    measures:
      - name: uniques
        sql: user_id
        type: count_distinct
    pre_aggregations:
      - name: original
        type: original_sql
  - name: users
    sql: SELECT 1
    dimensions:
      - name: id
        sql: id
        type: number
"#,
        "model.yml",
    )
    .unwrap();
    let orders = &model.cubes["orders"];
    assert_eq!(orders.joins[0].relationship.as_deref(), Some("manyToOne"));
    assert_eq!(
        orders.measures["uniques"].member_type.as_deref(),
        Some("countDistinct")
    );
    assert_eq!(
        orders.pre_aggregations["original"].pre_agg_type.as_deref(),
        Some("originalSql")
    );

    let config = meta_config(&model);
    let uniques = config.cubes[0].config["measures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "orders.uniques")
        .unwrap()
        .clone();
    assert_eq!(uniques["aggType"], "countDistinct");
    assert_eq!(uniques["type"], "number");
}

#[test]
fn pre_aggregation_type_defaults_to_rollup() {
    let model = ModelLoader::load_str(
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
    pre_aggregations:
      - name: main
        measures:
          - count
        dimensions:
          - status
"#,
        "model.yml",
    )
    .unwrap();
    assert_eq!(
        model.cubes["orders"].pre_aggregations["main"]
            .pre_agg_type
            .as_deref(),
        Some("rollup")
    );
}
