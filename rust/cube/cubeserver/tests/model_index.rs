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

