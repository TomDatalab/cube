//! Validation of a model write: the candidate sources are compiled in memory
//! and only problems that are new compared to the current model are refused.

use std::collections::BTreeSet;

use cubemodel::ownership::OwnershipPolicy;
use cubemodel::{ModelLoader, TemplateContext};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct Problem {
    pub file: String,
    pub cube: String,
    pub message: String,
}

/// Every compile error and ownership rejection of a set of sources.
#[derive(Debug, Clone, Default)]
pub struct Check {
    pub problems: BTreeSet<Problem>,
}

pub fn check(
    sources: &[(String, String)],
    ctx: &TemplateContext,
    policy: Option<&OwnershipPolicy>,
) -> Check {
    let mut problems = BTreeSet::new();
    let report = ModelLoader::with_context(ctx.clone()).check_sources_with_policy(sources, policy);
    for i in report.errors {
        problems.insert(Problem {
            file: i.file_name.clone().unwrap_or_default(),
            cube: String::new(),
            message: i.full_message(),
        });
    }
    for r in report.rejected {
        problems.insert(Problem {
            file: r.file,
            cube: r.cube,
            message: r.message,
        });
    }
    Check { problems }
}

/// Each cube and view name of `sources` with the file defining it.
pub fn definitions(sources: &[(String, String)]) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    for (path, content) in sources {
        if !(path.ends_with(".yml") || path.ends_with(".yaml")) {
            continue;
        }
        let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(content) else {
            continue;
        };
        for kind in ["cubes", "views"] {
            for item in doc.get(kind).and_then(|v| v.as_sequence()).into_iter().flatten() {
                if let Some(name) = item.get("name").and_then(|n| n.as_str()) {
                    out.entry(name.to_string()).or_insert_with(|| path.clone());
                }
            }
        }
    }
    out
}

/// Problems in `after` that `before` did not have, sorted.
pub fn new_problems(before: &Check, after: &Check) -> Vec<Problem> {
    after.problems.difference(&before.problems).cloned().collect()
}

/// Problems of `before` that `after` no longer has.
pub fn fixed_count(before: &Check, after: &Check) -> usize {
    before.problems.difference(&after.problems).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn src(files: &[(&str, &str)]) -> Vec<(String, String)> {
        files
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    fn run(files: &[(&str, &str)], policy: Option<&OwnershipPolicy>) -> Check {
        check(&src(files), &TemplateContext::default(), policy)
    }

    const A: &str = "cubes:\n  - name: a\n    sql_table: t\n";
    const B: &str = "cubes:\n  - name: b\n    sql_table: t\n";
    const BROKEN: &str = "cubes: [ {";

    #[test]
    fn valid_change_has_no_new_problems() {
        let before = run(&[("a.yml", A)], None);
        let after = run(&[("a.yml", A), ("b.yml", B)], None);
        assert!(new_problems(&before, &after).is_empty());
    }

    #[test]
    fn broken_yaml_is_new() {
        let before = run(&[("a.yml", A)], None);
        let after = run(&[("a.yml", A), ("b.yml", BROKEN)], None);
        let p = new_problems(&before, &after);
        assert_eq!(p.len(), 1, "{p:?}");
        assert_eq!(p[0].file, "b.yml");
    }

    #[test]
    fn old_errors_do_not_block_a_write() {
        let before = run(&[("a.yml", A), ("x.yml", BROKEN)], None);
        assert!(!before.problems.is_empty());
        let after = run(&[("a.yml", A), ("x.yml", BROKEN), ("b.yml", B)], None);
        assert!(new_problems(&before, &after).is_empty());
    }

    #[test]
    fn fixing_counts() {
        let before = run(&[("a.yml", A), ("x.yml", BROKEN)], None);
        let after = run(&[("a.yml", A)], None);
        assert_eq!(fixed_count(&before, &after), 1);
    }

    #[test]
    fn ownership_rejection_is_new() {
        let mut organizations = BTreeMap::new();
        organizations.insert(
            "on".to_string(),
            ["on_pg".to_string()].into_iter().collect(),
        );
        let policy = OwnershipPolicy {
            organizations,
            platform: BTreeSet::new(),
            generation: 1,
        };
        let x = "cubes:\n  - name: x\n    sql_table: t\n    data_source: sofiadb\n";
        let before = run(&[("a.yml", A)], Some(&policy));
        let after = run(&[("a.yml", A), ("on/x.yml", x)], Some(&policy));
        let p = new_problems(&before, &after);
        assert_eq!(p.len(), 1, "{p:?}");
        assert_eq!(p[0].cube, "x");
    }

    #[test]
    fn deleting_a_joined_cube_is_new() {
        let a = "cubes:\n  - name: a\n    sql_table: t\n    joins:\n      - name: b\n        sql: \"{CUBE}.id = {b}.id\"\n        relationship: many_to_one\n";
        let before = run(&[("a.yml", a), ("b.yml", B)], None);
        assert!(before.problems.is_empty(), "{:?}", before.problems);
        let after = run(&[("a.yml", a)], None);
        let p = new_problems(&before, &after);
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].message.contains('b'), "{p:?}");
    }

    fn on_policy() -> OwnershipPolicy {
        let mut organizations = BTreeMap::new();
        organizations.insert("on".to_string(), ["on_pg".to_string()].into_iter().collect());
        OwnershipPolicy {
            organizations,
            platform: BTreeSet::new(),
            generation: 1,
        }
    }

    #[test]
    fn old_compile_error_does_not_hide_a_rejection() {
        let policy = on_policy();
        let x = "cubes:\n  - name: x\n    sql_table: t\n    data_source: sofiadb\n";
        let before = run(&[("x.yml", BROKEN)], Some(&policy));
        let after = run(&[("x.yml", BROKEN), ("on/x.yml", x)], Some(&policy));
        let p = new_problems(&before, &after);
        assert_eq!(p.len(), 1, "{p:?}");
        assert_eq!(p[0].cube, "x");
    }

    #[test]
    fn old_template_error_does_not_hide_new_yaml() {
        let t = "{{ nope( }}";
        let before = run(&[("t.yml.jinja", t)], None);
        assert!(!before.problems.is_empty());
        let after = run(&[("t.yml.jinja", t), ("b.yml", BROKEN)], None);
        let p = new_problems(&before, &after);
        assert_eq!(p.len(), 1, "{p:?}");
        assert_eq!(p[0].file, "b.yml");
    }

    #[test]
    fn same_mistake_in_a_second_file_is_new() {
        let before = run(&[("x.yml", "- a")], None);
        let after = run(&[("x.yml", "- a"), ("b.yml", "- b")], None);
        let p = new_problems(&before, &after);
        assert_eq!(p.len(), 1, "{p:?}");
        assert_eq!(p[0].file, "b.yml");
    }
}
