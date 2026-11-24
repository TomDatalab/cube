//! The model index cache. The ownership policy is process-global, so these
//! live in a binary of their own, one at a time.

use std::sync::{Arc, Mutex};

use cubemodel::ownership::{set_policy, Owner, OwnershipPolicy};
use cubeserver::model_index::ModelIndexCache;

// The policy is process-wide: one test at a time, and cleared after.
static SERIAL: Mutex<()> = Mutex::new(());
struct Clear;
impl Drop for Clear {
    fn drop(&mut self) {
        set_policy(None);
    }
}

/// Installs a policy; `set_policy` gives it a new generation.
fn policy(orgs: &[(&str, &[&str])]) {
    set_policy(Some(Arc::new(OwnershipPolicy {
        organizations: orgs
            .iter()
            .map(|(s, d)| (s.to_string(), d.iter().map(|x| x.to_string()).collect()))
            .collect(),
        platform: Default::default(),
        generation: 0,
    })));
}

const CUBE: &str = "cubes:\n  - name: b\n    sql_table: t\n    data_source: sofiadb\n    measures:\n      - name: n\n        type: count\n";

#[test]
fn a_move_between_folders_rebuilds_the_index() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let _clear = Clear;
    policy(&[("digital", &["sofiadb"]), ("on", &["sofiadb"])]);
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("on")).unwrap();
    std::fs::create_dir_all(dir.path().join("digital")).unwrap();
    std::fs::write(dir.path().join("on/b.yml"), CUBE).unwrap();

    let cache = ModelIndexCache::default();
    let first = cache.get(dir.path());
    assert_eq!(first.cubes[0].owner, Owner::Organization("on".into()));
    assert!(Arc::ptr_eq(&first, &cache.get(dir.path())));

    // `rename` keeps the file's mtime.
    std::fs::rename(dir.path().join("on/b.yml"), dir.path().join("digital/b.yml")).unwrap();
    let second = cache.get(dir.path());
    assert_eq!(second.cubes[0].owner, Owner::Organization("digital".into()));
}

#[test]
fn a_new_policy_rebuilds_the_index() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let _clear = Clear;
    policy(&[("digital", &["sofiadb"])]);
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("digital")).unwrap();
    std::fs::write(dir.path().join("digital/b.yml"), CUBE).unwrap();

    let cache = ModelIndexCache::default();
    assert_eq!(cache.get(dir.path()).cubes.len(), 1);
    policy(&[("digital", &[])]);
    let index = cache.get(dir.path());
    assert!(index.cubes.is_empty());
    assert_eq!(index.rejected.len(), 1);
}


/// Digital's `a` on sofiadb, ON's `a` on ondb, and Platform's `u`.
fn two_orgs_model() -> (tempfile::TempDir, OwnershipPolicy) {
    let dir = tempfile::tempdir().unwrap();
    let a = |ds: &str| format!("cubes:\n  - name: a\n    sql_table: t\n    data_source: {ds}\n    measures:\n      - name: n\n        type: count\n");
    write(&dir, "digital/a.yml", &a("sofiadb"));
    write(&dir, "on/a.yml", &a("ondb"));
    write(&dir, "_platform/u.yml", "cubes:\n  - name: u\n    sql_table: t\n    measures:\n      - name: n\n        type: count\n");
    let policy = OwnershipPolicy {
        organizations: [("digital", "sofiadb"), ("on", "ondb")]
            .iter()
            .map(|(s, d)| (s.to_string(), [d.to_string()].into_iter().collect()))
            .collect(),
        platform: ["default".to_string()].into_iter().collect(),
        generation: 0,
    };
    (dir, policy)
}

fn write(dir: &tempfile::TempDir, name: &str, content: &str) {
    let path = dir.path().join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

#[test]
fn load_scoped_keeps_both_owners() {
    use cubeserver::model_index::ModelIndex;
    let (dir, policy) = two_orgs_model();
    let slugs = policy.organizations.keys().cloned().collect();
    let index = ModelIndex::load_scoped(dir.path(), Some(&policy), &slugs);
    let owners_of = |name: &str| -> Vec<Owner> {
        index.cubes.iter().filter(|c| c.name == name).map(|c| c.owner.clone()).collect()
    };
    let mut a_owners = owners_of("a");
    a_owners.sort_by_key(|o| format!("{o:?}"));
    assert_eq!(
        a_owners,
        [Owner::Organization("digital".into()), Owner::Organization("on".into())],
        "{:?}",
        index.errors
    );
    // Every scope compiles `_platform/`: its cube is listed once.
    assert_eq!(owners_of("u"), [Owner::Platform]);
    let digital = index
        .cubes
        .iter()
        .find(|c| c.owner == Owner::Organization("digital".into()))
        .unwrap();
    assert_eq!(digital.data_source, "sofiadb");
    assert_eq!(index.files.len(), 3);
    assert!(index.errors.is_empty() && index.rejected.is_empty());
}

#[test]
fn a_platform_error_is_kept_per_scope_and_reported_once() {
    use cubeserver::model_index::ModelIndex;
    let (dir, policy) = two_orgs_model();
    write(&dir, "_platform/bad.yml", "cubes:\n  - name: u\n    sql_table: t\n    measures:\n      - name: n\n        type: count\n");
    let slugs = policy.organizations.keys().cloned().collect();
    let index = ModelIndex::load_scoped(dir.path(), Some(&policy), &slugs);
    for key in ["digital", "on", "_platform"] {
        assert!(index.errors_in(key).count() >= 1, "{key}: {:?}", index.errors);
    }
    assert_eq!(index.unique_errors().len(), index.errors_in("on").count(), "{:?}", index.errors);
}
