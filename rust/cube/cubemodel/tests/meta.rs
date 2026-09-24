//! `/v1/meta` shape, visibility filtering and the golden model snapshot.

use cubemodel::{meta_config, rest_meta_response, DataModel, ModelLoader};
use serde_json::Value;

fn golden_model() -> DataModel {
    ModelLoader::load_dir("tests/fixtures/golden").unwrap()
}

/// Regenerate with `UPDATE_GOLDEN=1 cargo test -p cubemodel --test meta`.
#[test]
fn golden_rest_meta_response() {
    let model = golden_model();
    let body = rest_meta_response(&meta_config(&model), false);
    let actual = format!("{}\n", serde_json::to_string_pretty(&body).unwrap());
    let path = "tests/fixtures/golden_meta.json";

    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::write(path, &actual).unwrap();
    }

    let expected: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(body, expected, "meta response drifted:\n{actual}");
}

#[test]
fn meta_config_shape_per_cube() {
    let model = golden_model();
    let config = meta_config(&model);
    let orders = config.cubes.iter().find(|c| c.name == "orders").unwrap();
    let c = &orders.config;

    assert_eq!(c["name"], "orders");
    assert_eq!(c["type"], "cube");
    assert_eq!(c["title"], "Orders");
    assert_eq!(c["isVisible"], true);
    assert_eq!(c["public"], true);
    assert_eq!(c["description"], "All orders");
    assert_eq!(c["meta"]["owner"], "sales");
    // `orders` joins `users`, so both share a connected component.
    assert_eq!(c["connectedComponent"], 1);
    for key in [
        "measures",
        "dimensions",
        "segments",
        "hierarchies",
        "folders",
        "nestedFolders",
    ] {
        assert!(c[key].is_array(), "{key} should be an array");
    }

    let count = c["measures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "orders.count")
        .unwrap();
    assert_eq!(count["title"], "Orders Count");
    assert_eq!(count["shortTitle"], "Count");
    assert_eq!(count["type"], "number");
    assert_eq!(count["aggType"], "count");
    assert_eq!(count["cumulative"], false);
    assert_eq!(count["cumulativeTotal"], false);
    assert_eq!(
        count["drillMembersGrouped"]["dimensions"]
            .as_array()
            .unwrap()
            .len(),
        3
    );

    let amount = c["measures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "orders.total_amount")
        .unwrap();
    assert_eq!(amount["format"], "currency");
    assert_eq!(amount["currency"], "USD");
    assert_eq!(amount["formatDescription"]["name"], "currency");
    assert_eq!(amount["formatDescription"]["specifier"], "$,.2~f");
    assert_eq!(amount["formatDescription"]["currency"], "USD");
}

#[test]
fn views_are_typed_as_views_and_have_no_connected_component() {
    let config = meta_config(&golden_model());
    let view = config
        .cubes
        .iter()
        .find(|c| c.name == "orders_view")
        .unwrap();
    assert_eq!(view.config["type"], "view");
    assert!(view.config.get("connectedComponent").is_none());
    assert!(view.is_view);
}

#[test]
fn hidden_members_are_filtered_out_of_the_rest_response() {
    let model = golden_model();
    let config = meta_config(&model);

    // The config itself keeps the hidden member, flagged invisible.
    let orders = config.cubes.iter().find(|c| c.name == "orders").unwrap();
    let hidden = orders.config["dimensions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "orders.internal_note")
        .unwrap();
    assert_eq!(hidden["isVisible"], false);
    assert_eq!(hidden["public"], false);
    // Primary keys default to hidden.
    let id = orders.config["dimensions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "orders.id")
        .unwrap();
    assert_eq!(id["primaryKey"], true);
    assert_eq!(id["isVisible"], false);

    // The REST body drops them.
    let body = rest_meta_response(&config, false);
    let names: Vec<&str> = body["cubes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "orders")
        .unwrap()["dimensions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert!(!names.contains(&"orders.internal_note"));
    assert!(!names.contains(&"orders.id"));

    // Dev mode keeps them.
    let dev = cubemodel::meta::rest_meta_response_with(&config, false, true);
    let dev_names: Vec<&str> = dev["cubes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "orders")
        .unwrap()["dimensions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["name"].as_str().unwrap())
        .collect();
    assert!(dev_names.contains(&"orders.internal_note"));
}

#[test]
fn only_views_filters_the_response() {
    let config = meta_config(&golden_model());
    let body = rest_meta_response(&config, true);
    let cubes = body["cubes"].as_array().unwrap();
    assert_eq!(cubes.len(), 1);
    assert_eq!(cubes[0]["name"], "orders_view");
    assert_eq!(cubes[0]["type"], "view");
}

#[test]
fn cubes_without_visible_members_are_dropped() {
    let model = ModelLoader::load_str(
        r#"
cubes:
  - name: orders
    sql: SELECT 1
    public: false
    measures:
      - name: count
        type: count
  - name: users
    sql: SELECT 1
    dimensions:
      - name: city
        sql: city
        type: string
"#,
        "model.yml",
    )
    .unwrap();

    let body = rest_meta_response(&meta_config(&model), false);
    let names: Vec<&str> = body["cubes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["users"]);
}

#[test]
fn connected_components_follow_the_join_graph() {
    let model = ModelLoader::load_str(
        r#"
cubes:
  - name: a
    sql: SELECT 1
    joins:
      - name: b
        sql: "1 = 1"
        relationship: many_to_one
    measures:
      - name: count
        type: count
  - name: b
    sql: SELECT 1
    dimensions:
      - name: id
        sql: id
        type: number
  - name: c
    sql: SELECT 1
    joins:
      - name: d
        sql: "1 = 1"
        relationship: many_to_one
    measures:
      - name: count
        type: count
  - name: d
    sql: SELECT 1
    dimensions:
      - name: id
        sql: id
        type: number
  - name: lonely
    sql: SELECT 1
    measures:
      - name: count
        type: count
"#,
        "model.yml",
    )
    .unwrap();

    let config = meta_config(&model);
    let component = |name: &str| {
        config
            .cubes
            .iter()
            .find(|c| c.name == name)
            .unwrap()
            .config
            .get("connectedComponent")
            .cloned()
    };

    assert_eq!(component("a"), Some(serde_json::json!(1)));
    assert_eq!(component("b"), Some(serde_json::json!(1)));
    assert_eq!(component("c"), Some(serde_json::json!(2)));
    assert_eq!(component("d"), Some(serde_json::json!(2)));
    // A cube with no joins has no component id at all, like Node.
    assert_eq!(component("lonely"), None);
}

#[test]
fn granularities_and_formats_are_exposed() {
    let config = meta_config(&golden_model());
    let orders = config.cubes.iter().find(|c| c.name == "orders").unwrap();
    let created_at = orders.config["dimensions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "orders.created_at")
        .unwrap();
    let granularities = created_at["granularities"].as_array().unwrap();
    assert_eq!(granularities.len(), 1);
    assert_eq!(granularities[0]["name"], "fiscal_year");
    assert_eq!(granularities[0]["title"], "Fiscal Year");
    assert_eq!(granularities[0]["interval"], "1 year");
    assert_eq!(granularities[0]["origin"], "2024-02-01");

    let users = config.cubes.iter().find(|c| c.name == "users").unwrap();
    let age = users.config["dimensions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["name"] == "users.age")
        .unwrap();
    assert_eq!(age["format"]["type"], "custom-numeric");
    assert_eq!(age["format"]["value"], ",.1~f");
    assert_eq!(age["format"]["alias"], "number_1");
    assert_eq!(age["formatDescription"]["name"], "number_1");
    assert_eq!(age["formatDescription"]["specifier"], ",.1~f");
}
