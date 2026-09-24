//! Every YAML data model the planner's own test suite uses must load through
//! [`Model`], so the two crates keep agreeing on what the YAML loader accepts.

use cubeplanner::Model;
use std::path::PathBuf;

/// Models the YAML loader does not accept yet, with the reason. Empty: every
/// model in the planner's fixture corpus loads.
const KNOWN_GAPS: &[(&str, &str)] = &[];

#[test]
fn every_planner_fixture_model_loads() {
    let root = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../cubesqlplanner/cubesqlplanner/src/test_fixtures/schemas/yaml_files"
    ));

    let mut loaded = 0;
    let mut unexpected = Vec::new();

    for group in ["common", "compilation_tests", "symbol_evaluator"] {
        let dir = root.join(group);
        for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            let yaml = std::fs::read_to_string(&path).unwrap();

            match Model::from_yaml_str(&yaml) {
                Ok(model) => {
                    assert!(!model.cube_names().is_empty(), "{name} loaded no cubes");
                    loaded += 1;
                }
                Err(err) => {
                    if !KNOWN_GAPS.iter().any(|(gap, _)| *gap == name) {
                        unexpected.push(format!("{name}: {err}"));
                    }
                }
            }
        }
    }

    assert!(
        unexpected.is_empty(),
        "models failed to load: {unexpected:#?}"
    );
    assert!(loaded > 75, "expected the fixture corpus, loaded {loaded}");
}

/// A calc-group view declares members of its own instead of including cube
/// members - `views: [{name, dimensions: [...]}]` with no `cubes:` at all. The
/// loader used to reject the shape outright.
#[test]
fn a_view_that_declares_its_own_dimensions_loads() {
    let path = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../cubesqlplanner/cubesqlplanner/src/test_fixtures/schemas/yaml_files/common/\
         calc_groups_cross_join.yaml"
    ));
    let model = Model::from_yaml_str(&std::fs::read_to_string(&path).unwrap())
        .unwrap_or_else(|e| panic!("{e}"));

    assert_eq!(model.cube_names(), vec!["source", "source_a", "source_b"]);
}
