//! View resolution against the schema-compiler's own YAML fixtures.

use cubemodel::{meta_config, DataModel, ModelLoader};

fn load(fixture: &str) -> DataModel {
    let source = std::fs::read_to_string(format!("tests/fixtures/{fixture}")).unwrap();
    match ModelLoader::load_str(&source, fixture) {
        Ok(model) => model,
        Err(e) => panic!("{fixture} failed to load:\n{}", e.messages().join("\n")),
    }
}

fn hierarchy<'a>(
    model: &'a DataModel,
    view: &str,
    name: &str,
) -> &'a cubemodel::EvaluatedHierarchy {
    model.views[view]
        .evaluated_hierarchies
        .iter()
        .find(|h| h.name == name)
        .unwrap_or_else(|| panic!("no hierarchy {name} in {view}"))
}

#[test]
fn resolves_includes_star_and_aliases() {
    let model = load("hierarchies.yml");
    let view = &model.views["orders_users_view"];

    assert_eq!(
        view.dimensions.keys().cloned().collect::<Vec<_>>(),
        vec![
            "id",
            "number",
            "status",
            "city",
            "age",
            "state",
            "user_city"
        ]
    );
    assert_eq!(
        view.measures.keys().cloned().collect::<Vec<_>>(),
        vec!["count"]
    );

    // `alias: user_city` renames, and the alias member points back at the source.
    assert_eq!(
        view.dimensions["user_city"].alias_member.as_deref(),
        Some("users.city")
    );
    assert_eq!(
        view.dimensions["user_city"].sql.as_deref(),
        Some("users.city")
    );
    assert_eq!(
        view.measures["count"].alias_member.as_deref(),
        Some("orders.count")
    );
}

#[test]
fn resolves_prefixed_includes() {
    let model = load("hierarchies.yml");
    let view = &model.views["all_hierarchy_view"];
    for name in ["users_age", "users_state", "users_city"] {
        assert!(view.dimensions.contains_key(name), "missing {name}");
    }
    assert_eq!(
        view.dimensions["users_age"].alias_member.as_deref(),
        Some("users.age")
    );
}

#[test]
fn view_hierarchies_match_node() {
    let model = load("hierarchies.yml");

    let orders_hierarchy = hierarchy(&model, "orders_users_view", "orders_hierarchy");
    assert_eq!(orders_hierarchy.title.as_deref(), Some("Hello Hierarchy"));
    assert_eq!(
        orders_hierarchy.alias_member.as_deref(),
        Some("orders.orders_hierarchy")
    );
    assert_eq!(
        orders_hierarchy.levels,
        vec![
            "orders_users_view.status",
            "orders_users_view.number",
            "orders_users_view.user_city"
        ]
    );

    let other = hierarchy(&model, "orders_users_view", "some_other_hierarchy");
    assert_eq!(
        other.levels,
        vec!["orders_users_view.state", "orders_users_view.user_city"]
    );

    // `excludes` drops a hierarchy entirely.
    assert_eq!(
        model.views["orders_includes_excludes_view"]
            .evaluated_hierarchies
            .len(),
        1
    );
    // An empty `includes:` yields no hierarchies.
    assert_eq!(model.views["empty_view"].evaluated_hierarchies.len(), 0);

    // Prefixed hierarchies from a second cube.
    let view = &model.views["all_hierarchy_view"];
    assert_eq!(view.evaluated_hierarchies.len(), 3);
    let prefixed = hierarchy(&model, "all_hierarchy_view", "users_users_hierarchy");
    assert_eq!(
        prefixed.alias_member.as_deref(),
        Some("users.users_hierarchy")
    );
    assert_eq!(
        prefixed.levels,
        vec![
            "all_hierarchy_view.users_age",
            "all_hierarchy_view.users_city"
        ]
    );
}

#[test]
fn including_a_hierarchy_auto_includes_its_levels() {
    let model = load("hierarchies.yml");
    let view = &model.views["only_hierarchy_included_view"];
    for name in ["status", "number", "city"] {
        assert!(view.dimensions.contains_key(name), "missing {name}");
    }
}

#[test]
fn folders_flatten_and_nest_like_node() {
    let model = load("folders.yml");
    let config = meta_config(&model);
    let view = config
        .cubes
        .iter()
        .find(|c| c.name == "test_view4")
        .unwrap();

    let nested = view.config["nestedFolders"].as_array().unwrap();
    assert_eq!(nested.len(), 3);
    let folder3 = nested.iter().find(|f| f["name"] == "folder3").unwrap();
    let members = folder3["members"].as_array().unwrap();
    assert_eq!(members.len(), 3);
    assert_eq!(members[0], "test_view4.users_city");
    assert_eq!(members[1]["name"], "inner folder 4");
    assert_eq!(
        members[1]["members"].as_array().unwrap(),
        &vec![serde_json::json!("test_view4.renamed_orders_status")]
    );
    assert_eq!(members[2]["name"], "inner folder 5");
    assert_eq!(
        members[2]["members"]
            .as_array()
            .unwrap()
            .iter()
            .take(4)
            .cloned()
            .collect::<Vec<_>>(),
        vec![
            serde_json::json!("test_view4.renamed_orders_count"),
            serde_json::json!("test_view4.renamed_orders_id"),
            serde_json::json!("test_view4.renamed_orders_number"),
            serde_json::json!("test_view4.renamed_orders_status"),
        ]
    );

    // Nested folder members are merged into the flat root folder.
    let flat = view.config["folders"].as_array().unwrap();
    let flat3 = flat.iter().find(|f| f["name"] == "folder3").unwrap();
    let flat_members: Vec<&str> = flat3["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap())
        .collect();
    assert!(flat_members.contains(&"test_view4.users_city"));
    assert!(flat_members.contains(&"test_view4.renamed_orders_status"));
}

#[test]
fn folders_accept_join_path_includes() {
    let model = load("folders.yml");
    let config = meta_config(&model);
    let view = config
        .cubes
        .iter()
        .find(|c| c.name == "test_view_join_path")
        .unwrap();
    let flat = view.config["folders"].as_array().unwrap();
    assert_eq!(flat.len(), 4);

    let members = |name: &str| -> Vec<String> {
        flat.iter().find(|f| f["name"] == name).unwrap()["members"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m.as_str().unwrap().to_string())
            .collect()
    };

    for expected in [
        "test_view_join_path.orders_count",
        "test_view_join_path.orders_id",
        "test_view_join_path.orders_number",
        "test_view_join_path.orders_status",
    ] {
        assert!(members("Orders Folder").contains(&expected.to_string()));
    }
    for expected in [
        "test_view_join_path.addresses_street",
        "test_view_join_path.addresses_zip_code",
    ] {
        assert!(members("Addresses Folder").contains(&expected.to_string()));
    }
    let mixed = members("Mixed Folder");
    assert!(mixed.contains(&"test_view_join_path.users_gender".to_string()));
    assert!(mixed.contains(&"test_view_join_path.orders_count".to_string()));
}

#[test]
fn views_extending_views_inherit_cubes_and_folders() {
    let model = load("folders.yml");
    let view = &model.views["test_view3"];
    // From test_view2 (inherited) plus its own users include.
    assert!(view.dimensions.contains_key("renamed_orders_status"));
    assert!(view.dimensions.contains_key("users_age"));
    assert!(view.dimensions.contains_key("users_city"));
    assert!(view
        .dimensions
        .contains_key("users_renamed_in_view3_gender"));
    assert_eq!(view.evaluated_folders.len(), 2);
}

#[test]
fn drill_members_are_remapped_into_the_view() {
    let model = ModelLoader::load_dir("tests/fixtures/golden").unwrap();
    let view = &model.views["orders_view"];
    let config = meta_config(&model);
    let view_config = config
        .cubes
        .iter()
        .find(|c| c.name == "orders_view")
        .unwrap();
    let count = view_config.config["measures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "orders_view.count")
        .unwrap();
    assert_eq!(
        count["drillMembers"].as_array().unwrap(),
        &vec![
            serde_json::json!("orders_view.id"),
            serde_json::json!("orders_view.status"),
            serde_json::json!("orders_view.users_city"),
        ]
    );
    assert!(view.measures.contains_key("count"));
}

#[test]
fn excluded_members_do_not_reach_the_view() {
    let model = ModelLoader::load_dir("tests/fixtures/golden").unwrap();
    let view = &model.views["orders_view"];
    assert!(!view.dimensions.contains_key("internal_note"));
    assert!(view.dimensions.contains_key("status"));
}
