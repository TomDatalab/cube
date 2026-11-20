use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use cubemodel::ownership::{set_policy, Owner, OwnershipPolicy, Rejection};
use cubemodel::{DataModel, ModelLoader};

static LOCK: Mutex<()> = Mutex::new(());

fn set(xs: &[&str]) -> BTreeSet<String> {
    xs.iter().map(|s| s.to_string()).collect()
}

fn policy() -> OwnershipPolicy {
    let mut organizations = BTreeMap::new();
    organizations.insert("digital".to_string(), set(&["sofiadb", "example"]));
    organizations.insert("on".to_string(), set(&["on_pg"]));
    OwnershipPolicy {
        organizations,
        platform: set(&["default"]),
        generation: 1,
    }
}

fn load(p: Option<OwnershipPolicy>, files: &[(&str, &str)]) -> DataModel {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_policy(p.map(Arc::new));
    let sources: Vec<(String, String)> = files
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
    let r = ModelLoader::new()
        .without_validation()
        .load_sources(&sources);
    set_policy(None);
    r.expect("model loads")
}

fn cube(name: &str, ds: Option<&str>, extra: &str) -> String {
    let ds = ds
        .map(|d| format!("    data_source: {d}\n"))
        .unwrap_or_default();
    format!("cubes:\n  - name: {name}\n    sql_table: t\n{ds}{extra}")
}

fn rej(file: &str, cube: &str, message: &str) -> Rejection {
    Rejection {
        file: file.into(),
        cube: cube.into(),
        message: message.into(),
    }
}

#[test]
fn owner_of_file_cases() {
    let p = policy();
    assert_eq!(
        p.owner_of_file("digital/sofiadb/x.yml"),
        Owner::Organization("digital".into())
    );
    assert_eq!(p.owner_of_file("_platform/u.yml"), Owner::Platform);
    assert_eq!(p.owner_of_file("orders.yml"), Owner::Unowned("".into()));
    assert_eq!(p.owner_of_file("zzz/a.yml"), Owner::Unowned("zzz".into()));
}

#[test]
fn a_cube_on_its_orgs_connection_is_kept() {
    let m = load(
        Some(policy()),
        &[("digital/a.yml", &cube("a", Some("sofiadb"), ""))],
    );
    assert!(m.cubes.contains_key("a"));
    assert!(m.rejected.is_empty());
}

#[test]
fn a_cube_on_another_orgs_connection_is_rejected() {
    let m = load(
        Some(policy()),
        &[("on/b.yml", &cube("b", Some("sofiadb"), ""))],
    );
    assert_eq!(
        m.rejected,
        vec![rej(
            "on/b.yml",
            "b",
            "data source sofiadb is not a connection of on"
        )]
    );
    assert!(!m.cubes.contains_key("b"));
}

#[test]
fn no_data_source_means_default() {
    let m = load(
        Some(policy()),
        &[
            ("_platform/u.yml", &cube("u", None, "")),
            ("digital/u2.yml", &cube("u2", None, "")),
        ],
    );
    assert!(m.cubes.contains_key("u"));
    assert_eq!(
        m.rejected,
        vec![rej(
            "digital/u2.yml",
            "u2",
            "data source default is not a connection of digital"
        )]
    );
}

#[test]
fn an_org_data_source_in_platform_is_rejected() {
    let m = load(
        Some(policy()),
        &[("_platform/f.yml", &cube("f", Some("sofiadb"), ""))],
    );
    assert_eq!(
        m.rejected,
        vec![rej(
            "_platform/f.yml",
            "f",
            "data source sofiadb belongs to an organization; move this file to its folder"
        )]
    );
}

#[test]
fn root_and_unknown_folders_are_rejected() {
    let m = load(
        Some(policy()),
        &[
            ("orders.yml", &cube("o", None, "")),
            ("zzz/a.yml", &cube("z", None, "")),
        ],
    );
    assert_eq!(
        m.rejected,
        vec![
            rej(
                "orders.yml",
                "o",
                "the folder / does not belong to any organization"
            ),
            rej(
                "zzz/a.yml",
                "z",
                "the folder zzz/ does not belong to any organization"
            ),
        ]
    );
}

#[test]
fn a_join_across_organizations_rejects_the_joining_cube() {
    let a = cube(
        "a",
        Some("sofiadb"),
        "    joins:\n      - name: c\n        sql: 1=1\n        relationship: many_to_one\n",
    );
    let c = cube("c", Some("on_pg"), "");
    let m = load(Some(policy()), &[("digital/a.yml", &a), ("on/c.yml", &c)]);
    assert_eq!(
        m.rejected,
        vec![rej("digital/a.yml", "a", "joins c belongs to on")]
    );
    assert!(m.cubes.contains_key("c"));
}

#[test]
fn a_view_including_another_orgs_cube_is_rejected() {
    let v = "views:\n  - name: v\n    data_source: sofiadb\n    cubes:\n      - join_path: c\n        includes: \"*\"\n";
    let c = cube("c", Some("on_pg"), "");
    let m = load(Some(policy()), &[("digital/v.yml", v), ("on/c.yml", &c)]);
    assert_eq!(
        m.rejected,
        vec![rej("digital/v.yml", "v", "includes c belongs to on")]
    );
    assert!(!m.views.contains_key("v"));
}

#[test]
fn rejection_cascades_once() {
    let a = cube(
        "a",
        Some("sofiadb"),
        "    joins:\n      - name: b\n        sql: 1=1\n        relationship: many_to_one\n",
    );
    let b = cube(
        "b",
        Some("on_pg"),
        "    joins:\n      - name: a\n        sql: 1=1\n        relationship: many_to_one\n",
    );
    let m = load(
        Some(policy()),
        &[("digital/a.yml", &a), ("digital/b.yml", &b)],
    );
    assert_eq!(
        m.rejected,
        vec![
            rej("digital/a.yml", "a", "depends on rejected cube b"),
            rej(
                "digital/b.yml",
                "b",
                "data source on_pg is not a connection of digital"
            ),
        ]
    );
}

#[test]
fn the_rest_of_the_model_still_compiles() {
    let m = load(
        Some(policy()),
        &[
            ("digital/a.yml", &cube("a", Some("sofiadb"), "")),
            ("zzz/b.yml", &cube("b", None, "")),
        ],
    );
    assert!(m.cubes.contains_key("a"));
    assert_eq!(m.rejected.len(), 1);
}

#[test]
fn no_policy_changes_nothing() {
    let m = load(None, &[("orders.yml", &cube("o", None, ""))]);
    assert!(m.cubes.contains_key("o"));
    assert!(m.rejected.is_empty());
}
