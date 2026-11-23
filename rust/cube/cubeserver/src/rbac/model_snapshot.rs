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

/// Where the import keeps a file that has no owner (one at the root of the
/// folder, or in a folder of no organization): `_unowned/<its path>`, with no
/// organization. The ownership rule rejects its cubes.
pub const UNOWNED_DIR: &str = "_unowned";

/// The source of a file with no organization: `_platform/<path>`, or the path
/// itself for an unowned one (`_unowned/...`).
pub fn platform_source(path: &str) -> String {
    match path.strip_prefix(UNOWNED_DIR) {
        Some(rest) if rest.starts_with('/') => path.to_string(),
        _ => format!("_platform/{path}"),
    }
}

/// `<slug>/<path>`, or for a file with no organization [`platform_source`].
pub fn source_path(file: &ModelFile, slugs: &HashMap<Uuid, String>) -> Option<String> {
    match file.organization_id {
        None => Some(platform_source(&file.path)),
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

/// Keeps the two highest versions and whatever `live` points to.
fn prune(cache_dir: &Path) -> io::Result<()> {
    let target = std::fs::read_link(cache_dir.join("live")).ok();
    let mut versions: Vec<(i64, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(cache_dir)? {
        let entry = entry?;
        if let Some(v) = entry.file_name().to_str().and_then(version_of) {
            versions.push((v, entry.path()));
        }
    }
    versions.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in versions.into_iter().skip(2) {
        if target.as_deref().and_then(Path::file_name) == path.file_name() {
            continue;
        }
        std::fs::remove_dir_all(path)?;
    }
    Ok(())
}

/// Creates `dir` (mode 0700) if missing, then checks it is a real directory
/// owned by this user with no group/other permissions.
pub fn prepare_cache_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    let denied = |why: &str| {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("model cache dir {}: {why}", dir.display()),
        )
    };
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let meta = std::fs::symlink_metadata(dir)?;
    if !meta.file_type().is_dir() {
        return Err(denied("not a real directory"));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(denied("has group/other permissions"));
    }
    // The uid of a file we create here is our effective uid.
    let probe = dir.join(unique_tmp(".owner-probe"));
    std::fs::write(&probe, b"")?;
    let me = std::fs::symlink_metadata(&probe).map(|m| m.uid());
    let _ = std::fs::remove_file(&probe);
    if me? != meta.uid() {
        return Err(denied("not owned by the current user"));
    }
    Ok(())
}

/// A name no other call of this process or of another one uses.
fn unique_tmp(base: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    format!("{base}.tmp-{}-{n}", std::process::id())
}

/// Whether `name` is a temporary entry of [`materialize`]: `v<n>.tmp…` or
/// `live.tmp…`.
fn is_tmp(name: &str) -> bool {
    match name.split_once(".tmp") {
        Some(("live", _)) => true,
        Some((v, _)) => version_of(v).is_some(),
        None => false,
    }
}

/// Removes the temporary entries a stopped [`materialize`] left behind. Only
/// call it while no materialize runs on `cache_dir`: it would remove that
/// one's half-written folder.
pub fn remove_stale_tmp(cache_dir: &Path) -> io::Result<()> {
    let entries = match std::fs::read_dir(cache_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_name().to_str().is_some_and(is_tmp) {
            continue;
        }
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(path)?;
        } else {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}

/// Writes `v<version>/` (through a temporary folder of its own), points
/// `live` at it with an atomic rename and prunes old versions (never `live`'s
/// target). An existing `v<version>` is complete by construction, so only
/// `live` is relinked. Prune errors are logged, not returned. Temporary
/// names are unique per call, so two calls never write into the same folder.
pub fn materialize(
    cache_dir: &Path,
    version: i64,
    sources: &[(String, String)],
) -> io::Result<PathBuf> {
    materialize_with(cache_dir, version, sources, prune)
}

fn materialize_with(
    cache_dir: &Path,
    version: i64,
    sources: &[(String, String)],
    prune: impl Fn(&Path) -> io::Result<()>,
) -> io::Result<PathBuf> {
    prepare_cache_dir(cache_dir)?;
    let name = format!("v{version}");
    let dir = cache_dir.join(&name);
    if !dir.is_dir() {
        let tmp = cache_dir.join(unique_tmp(&name));
        std::fs::create_dir(&tmp)?;
        if let Err(e) = write_sources(&tmp, sources) {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(e);
        }
        if let Err(e) = std::fs::rename(&tmp, &dir) {
            let _ = std::fs::remove_dir_all(&tmp);
            // Another call completed the same version first.
            if !dir.is_dir() {
                return Err(e);
            }
        }
    }
    let live = cache_dir.join("live");
    let live_tmp = cache_dir.join(unique_tmp("live"));
    std::os::unix::fs::symlink(&name, &live_tmp)?;
    if let Err(e) = std::fs::rename(&live_tmp, &live) {
        let _ = std::fs::remove_file(&live_tmp);
        return Err(e);
    }
    if let Err(e) = prune(cache_dir) {
        tracing::warn!(error = %e, "could not prune old model versions");
    }
    Ok(live)
}

/// `CUBE_MODEL_CACHE_DIR`, or `<temp>/cube-model-<port>`. Pure: call
/// [`prepare_cache_dir`] (materialize does) before using it.
pub fn cache_dir_from_env(port: u16) -> PathBuf {
    match std::env::var_os("CUBE_MODEL_CACHE_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => std::env::temp_dir().join(format!("cube-model-{port}")),
    }
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

    fn tdir() -> tempfile::TempDir {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        std::fs::set_permissions(d.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        d
    }

    fn src(c: &str) -> Vec<(String, String)> {
        vec![("digital/fatos.yml".into(), c.into())]
    }

    #[test]
    fn validate_path_rules() {
        for ok in ["fatos.yml", "sofiadb/dfos_a.yml", "x.J2", "_unowned/x.yml", "_unowned/ghost/y.yml"] {
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
        let unowned = file(None, "_unowned/ghost/x.yml", "");
        assert_eq!(source_path(&unowned, &slugs).as_deref(), Some("_unowned/ghost/x.yml"));
        let lookalike = file(None, "_unownedx/x.yml", "");
        assert_eq!(source_path(&lookalike, &slugs).as_deref(), Some("_platform/_unownedx/x.yml"));
        let orphan = file(Some(Uuid::new_v4()), "x.yml", "");
        assert_eq!(source_path(&orphan, &slugs), None);
    }

    #[test]
    fn materialize_writes_and_links() {
        let d = tdir();
        let live = materialize(d.path(), 3, &src("hello")).unwrap();
        assert_eq!(live, d.path().join("live"));
        assert_eq!(std::fs::read_link(&live).unwrap(), PathBuf::from("v3"));
        assert_eq!(std::fs::read_to_string(live.join("digital/fatos.yml")).unwrap(), "hello");
    }

    #[test]
    fn materialize_prunes_old_versions() {
        let d = tdir();
        for v in 1..=3 {
            materialize(d.path(), v, &src("x")).unwrap();
        }
        assert!(!d.path().join("v1").exists());
        assert!(d.path().join("v2").exists());
        assert!(d.path().join("v3").exists());
    }

    #[test]
    fn live_switches_only_to_complete_versions() {
        let d = tdir();
        materialize(d.path(), 4, &src("old")).unwrap();
        let tmp = d.path().join("v5.tmp");
        std::fs::create_dir_all(tmp.join("digital")).unwrap();
        std::fs::write(tmp.join("partial.yml"), "half").unwrap();
        std::fs::write(d.path().join("live.tmp-1-1"), "").unwrap();
        assert_eq!(std::fs::read_link(d.path().join("live")).unwrap(), PathBuf::from("v4"));
        remove_stale_tmp(d.path()).unwrap();
        assert!(!d.path().join("live.tmp-1-1").exists());
        let live = materialize(d.path(), 5, &src("new")).unwrap();
        assert_eq!(std::fs::read_link(&live).unwrap(), PathBuf::from("v5"));
        assert_eq!(std::fs::read_to_string(live.join("digital/fatos.yml")).unwrap(), "new");
        assert!(!live.join("partial.yml").exists());
        assert!(!tmp.exists());
    }

    #[test]
    fn concurrent_materialize_of_one_version_is_complete() {
        let d = tdir();
        let srcs: Vec<(String, String)> =
            (0..300).map(|i| (format!("digital/f{i}.yml"), format!("c{i}"))).collect();
        std::thread::scope(|s| {
            for _ in 0..8 {
                s.spawn(|| materialize(d.path(), 7, &srcs).unwrap());
            }
        });
        let live = d.path().join("live");
        assert_eq!(std::fs::read_link(&live).unwrap(), PathBuf::from("v7"));
        for (p, c) in &srcs {
            assert_eq!(&std::fs::read_to_string(live.join(p)).unwrap(), c);
        }
        let left: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }

    #[test]
    fn stale_tmp_names() {
        for t in ["v5.tmp", "v5.tmp-1-2", "live.tmp", "live.tmp-9-0"] {
            assert!(is_tmp(t), "{t}");
        }
        for n in ["v5", "live", "digital.tmp", "x.tmp-1"] {
            assert!(!is_tmp(n), "{n}");
        }
    }

    #[test]
    fn editable_only_plain_yaml() {
        assert!(is_editable("a.yml", None));
        assert!(!is_editable("a.jinja", None));
        assert!(!is_editable("a.yml", Some("sofiadb-sync")));
    }

    #[test]
    fn prune_keeps_live_target_when_version_goes_back() {
        let d = tdir();
        for v in [5, 6, 7, 3] {
            materialize(d.path(), v, &src("x")).unwrap();
        }
        let live = d.path().join("live");
        assert_eq!(std::fs::read_link(&live).unwrap(), PathBuf::from("v3"));
        assert!(live.join("digital/fatos.yml").is_file());
        assert!(d.path().join("v3").exists());
    }

    #[test]
    fn prune_failure_does_not_fail_materialize() {
        let d = tdir();
        let live = materialize_with(d.path(), 2, &src("x"), |_| {
            Err(io::Error::new(io::ErrorKind::Other, "boom"))
        })
        .unwrap();
        assert_eq!(std::fs::read_link(&live).unwrap(), PathBuf::from("v2"));
    }

    #[test]
    fn prepare_refuses_a_symlink() {
        let d = tdir();
        let real = d.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = d.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(prepare_cache_dir(&link).is_err());
    }

    #[test]
    fn prepare_refuses_group_writable() {
        use std::os::unix::fs::PermissionsExt;
        let d = tdir();
        let c = d.path().join("c");
        std::fs::create_dir(&c).unwrap();
        std::fs::set_permissions(&c, std::fs::Permissions::from_mode(0o770)).unwrap();
        let e = prepare_cache_dir(&c).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn prepare_creates_with_0700() {
        use std::os::unix::fs::PermissionsExt;
        let d = tdir();
        let c = d.path().join("a/b");
        prepare_cache_dir(&c).unwrap();
        let mode = std::fs::metadata(&c).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }
}
