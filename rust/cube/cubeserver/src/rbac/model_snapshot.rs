//! Turns the model rows into compile sources and writes them to a private
//! folder per model version. `live` is a symlink swapped atomically, so it
//! never points at an incomplete version.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use uuid::Uuid;

use super::model_store::{ModelFile, ModelRows};

const MAX_PATH_LEN: usize = 255;
const EXTENSIONS: [&str; 4] = ["yml", "yaml", "jinja", "j2"];

fn extension(path: &str) -> Option<String> {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())
}

/// Checks a file path relative to its organization.
pub fn validate_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("path is empty".into());
    }
    if path.chars().count() > MAX_PATH_LEN {
        return Err(format!("path is longer than {MAX_PATH_LEN} characters"));
    }
    for seg in path.split('/') {
        if seg.is_empty() {
            return Err("path has an empty component or a leading/trailing slash".into());
        }
        if seg == "." || seg == ".." {
            return Err("path must not contain . or .. components".into());
        }
        if !seg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return Err(format!("invalid characters in path component '{seg}'"));
        }
    }
    match extension(path) {
        Some(e) if EXTENSIONS.contains(&e.as_str()) => Ok(()),
        _ => Err("extension must be .yml, .yaml, .jinja or .j2".into()),
    }
}

/// Only plain YAML files not managed by a sync can be edited by hand.
pub fn is_editable(path: &str, managed_by: Option<&str>) -> bool {
    let yaml = matches!(extension(path).as_deref(), Some("yml" | "yaml"));
    yaml && managed_by.map_or(true, str::is_empty)
}

/// `<slug>/<path>`, or `_platform/<path>` for platform files.
pub fn source_path(file: &ModelFile, slugs: &HashMap<Uuid, String>) -> Option<String> {
    match file.organization_id {
        None => Some(format!("_platform/{}", file.path)),
        Some(org) => slugs.get(&org).map(|s| format!("{s}/{}", file.path)),
    }
}

/// `(source_path, content)` for every file whose organization has a slug,
/// ordered by path.
pub fn sources(rows: &ModelRows, slugs: &HashMap<Uuid, String>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = rows
        .files
        .iter()
        .filter_map(|f| source_path(f, slugs).map(|p| (p, f.content.clone())))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, msg)
}

fn write_sources(dir: &Path, sources: &[(String, String)]) -> io::Result<()> {
    for (path, content) in sources {
        let safe = !path.is_empty()
            && !path.starts_with('/')
            && !path.contains('\\')
            && path
                .split('/')
                .all(|s| !s.is_empty() && s != "." && s != "..");
        if !safe {
            return Err(invalid(format!("unsafe source path '{path}'")));
        }
        let target = dir.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(target, content)?;
    }
    Ok(())
}

fn version_of(name: &str) -> Option<i64> {
    let digits = name.strip_prefix('v')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn prune(cache_dir: &Path) -> io::Result<()> {
    let mut versions: Vec<(i64, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(cache_dir)? {
        let entry = entry?;
        if let Some(v) = entry.file_name().to_str().and_then(version_of) {
            versions.push((v, entry.path()));
        }
    }
    versions.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in versions.into_iter().skip(2) {
        std::fs::remove_dir_all(path)?;
    }
    Ok(())
}

/// Writes `v<version>/` (through `v<version>.tmp/`), points `live` at it with
/// an atomic rename and prunes all but the two newest versions. An existing
/// `v<version>` is complete by construction, so only `live` is relinked.
pub fn materialize(
    cache_dir: &Path,
    version: i64,
    sources: &[(String, String)],
) -> io::Result<PathBuf> {
    std::fs::create_dir_all(cache_dir)?;
    let name = format!("v{version}");
    let dir = cache_dir.join(&name);
    if !dir.is_dir() {
        let tmp = cache_dir.join(format!("{name}.tmp"));
        if tmp.exists() {
            std::fs::remove_dir_all(&tmp)?;
        }
        std::fs::create_dir(&tmp)?;
        if let Err(e) = write_sources(&tmp, sources) {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(e);
        }
        std::fs::rename(&tmp, &dir)?;
    }
    let live = cache_dir.join("live");
    let live_tmp = cache_dir.join("live.tmp");
    if live_tmp.symlink_metadata().is_ok() {
        std::fs::remove_file(&live_tmp)?;
    }
    std::os::unix::fs::symlink(&name, &live_tmp)?;
    std::fs::rename(&live_tmp, &live)?;
    prune(cache_dir)?;
    Ok(live)
}

/// `CUBE_MODEL_CACHE_DIR`, or `<temp>/cube-model-<port>`; created with mode
/// 0700 (best effort: a failure surfaces when materializing).
pub fn cache_dir_from_env(port: u16) -> PathBuf {
    let dir = match std::env::var_os("CUBE_MODEL_CACHE_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => std::env::temp_dir().join(format!("cube-model-{port}")),
    };
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir);
    let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    dir
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn file(org: Option<Uuid>, path: &str, content: &str) -> ModelFile {
        ModelFile {
            id: Uuid::new_v4(),
            organization_id: org,
            path: path.into(),
            content: content.into(),
            version: 1,
            managed_by: None,
            created_at: Utc::now(),
            created_by: "t".into(),
            updated_at: Utc::now(),
            updated_by: "t".into(),
        }
    }

    fn src(c: &str) -> Vec<(String, String)> {
        vec![("digital/fatos.yml".into(), c.into())]
    }

    #[test]
    fn validate_path_rules() {
        for ok in ["fatos.yml", "sofiadb/dfos_a.yml", "x.J2"] {
            assert!(validate_path(ok).is_ok(), "{ok}");
        }
        let long = format!("{}.yml", "a".repeat(252));
        assert_eq!(long.len(), 256);
        for bad in ["../a.yml", "/a.yml", "a//b.yml", "a\\b.yml", "a.txt", "a b.yml", &long] {
            assert!(validate_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn sources_prefix_each_owner() {
        let a = Uuid::new_v4();
        let slugs = HashMap::from([(a, "digital".to_string())]);
        let rows = ModelRows {
            version: 1,
            files: vec![file(Some(a), "fatos.yml", "a"), file(None, "fatos.yml", "p")],
        };
        let paths: Vec<_> = sources(&rows, &slugs).into_iter().map(|s| s.0).collect();
        assert_eq!(paths, ["_platform/fatos.yml", "digital/fatos.yml"]);
        let orphan = file(Some(Uuid::new_v4()), "x.yml", "");
        assert_eq!(source_path(&orphan, &slugs), None);
    }

    #[test]
    fn materialize_writes_and_links() {
        let d = tempfile::tempdir().unwrap();
        let live = materialize(d.path(), 3, &src("hello")).unwrap();
        assert_eq!(live, d.path().join("live"));
        assert_eq!(std::fs::read_link(&live).unwrap(), PathBuf::from("v3"));
        assert_eq!(std::fs::read_to_string(live.join("digital/fatos.yml")).unwrap(), "hello");
    }

    #[test]
    fn materialize_prunes_old_versions() {
        let d = tempfile::tempdir().unwrap();
        for v in 1..=3 {
            materialize(d.path(), v, &src("x")).unwrap();
        }
        assert!(!d.path().join("v1").exists());
        assert!(d.path().join("v2").exists());
        assert!(d.path().join("v3").exists());
    }

    #[test]
    fn live_switches_only_to_complete_versions() {
        let d = tempfile::tempdir().unwrap();
        materialize(d.path(), 4, &src("old")).unwrap();
        let tmp = d.path().join("v5.tmp");
        std::fs::create_dir_all(tmp.join("digital")).unwrap();
        std::fs::write(tmp.join("partial.yml"), "half").unwrap();
        assert_eq!(std::fs::read_link(d.path().join("live")).unwrap(), PathBuf::from("v4"));
        let live = materialize(d.path(), 5, &src("new")).unwrap();
        assert_eq!(std::fs::read_link(&live).unwrap(), PathBuf::from("v5"));
        assert_eq!(std::fs::read_to_string(live.join("digital/fatos.yml")).unwrap(), "new");
        assert!(!live.join("partial.yml").exists());
        assert!(!tmp.exists());
    }

    #[test]
    fn editable_only_plain_yaml() {
        assert!(is_editable("a.yml", None));
        assert!(!is_editable("a.jinja", None));
        assert!(!is_editable("a.yml", Some("sofiadb-sync")));
    }
}
