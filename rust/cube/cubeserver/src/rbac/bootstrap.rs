//! With RBAC on, the data sources come from its database: at start-up,
//! before anything reads them, the ones `cube.yml` still declares are
//! imported once (and taken out of `cube.yml` and `.env`, which are backed
//! up first), then every stored one replaces the file's.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use cubeconfig::data_source::key_by_data_source;
use cubeconfig::{data_source_from_variables, CubeConfig, Env};

use super::data_sources::{load_all, reseal_unbound, save};
use super::secrets::SecretKey;
use super::store::Store;
use crate::orchestrator_adapter::MANAGED_FIELDS;
use crate::handlers::data_sources::{
    current_variables, env_keys, env_with_file, header, own_setting, remove_from_config, update_env,
    write_atomically,
};

#[derive(Default)]
pub struct ImportReport {
    /// Data sources copied from the files this time.
    pub imported: Vec<String>,
    /// Data sources loaded from RBAC.
    pub loaded: usize,
    /// Every stored setting as the variable a driver reads
    /// (`CUBEJS_DS_<NAME>_*`, `CUBEJS_DB_*` for `default`).
    pub env: HashMap<String, String>,
}

/// `env`'s names only: its values (passwords) never reach a log.
impl std::fmt::Debug for ImportReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut env: Vec<&String> = self.env.keys().collect();
        env.sort();
        f.debug_struct("ImportReport")
            .field("imported", &self.imported)
            .field("loaded", &self.loaded)
            .field("env", &env)
            .finish()
    }
}

/// The variables of the process environment RBAC supersedes: the ones it
/// holds a value for (`report.env`) and the settings the Data Source page
/// manages of every stored data source (which never fall back to the
/// environment). They are dropped from the environment so a password
/// moved into RBAC does not stay readable in `/proc/<pid>/environ`.
pub fn superseded_env(
    report: &ImportReport,
    names: &[String],
    present: impl Fn(&str) -> bool,
) -> Vec<String> {
    let managed = names
        .iter()
        .flat_map(|name| MANAGED_FIELDS.iter().map(move |field| key_by_data_source(field, name)));
    let mut keys: Vec<String> = report
        .env
        .keys()
        .cloned()
        .chain(managed)
        .filter(|key| present(key))
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// At start-up: [`data_sources_from_rbac`] with the deployment's
/// `cube.yml` (if any) and its `.env` under the process environment.
pub async fn load_at_startup(
    config: &super::RbacConfig,
    cube_config: &mut CubeConfig,
) -> Result<ImportReport, String> {
    let store = Store::connect(&config.db_url)
        .await
        .map_err(|e| format!("cannot reach the RBAC database: {e}"))?;
    let file = CubeConfig::find(Path::new(".")).ok();
    let env = match &file {
        Some(file) => env_with_file(&file.with_file_name(".env")),
        None => Env::from_process(),
    };
    data_sources_from_rbac(&store, &config.secret_key, cube_config, file.as_deref(), &env).await
}

pub async fn data_sources_from_rbac(
    store: &Store,
    key: &SecretKey,
    cube_config: &mut CubeConfig,
    config_file: Option<&Path>,
    env: &Env,
) -> Result<ImportReport, String> {
    let mut report = ImportReport::default();

    // Secrets sealed before they were bound to their row.
    for name in reseal_unbound(store, key).await? {
        eprintln!("🔑 data source {name}: its secrets were sealed again, bound to it");
    }

    // What the files declare and RBAC does not have yet, plus `default`
    // from the `CUBEJS_DB_*` variables when nothing declares it (an
    // environment-only deployment).
    let stored: BTreeSet<String> = store
        .data_sources()
        .await
        .map_err(|e| format!("cannot read the data sources: {e}"))?
        .into_iter()
        .map(|row| row.name)
        .collect();
    let mut candidates: Vec<(String, BTreeMap<String, String>)> = Vec::new();
    for name in cube_config.data_sources.keys() {
        if !stored.contains(name) {
            let variables = current_variables(cube_config, name, env)
                .map_err(|e| format!("data source {name}: {}", e.body.error))?;
            candidates.push((name.clone(), own_settings(name, variables)));
        }
    }
    if !stored.contains("default") && !cube_config.data_sources.contains_key("default") {
        if let Ok(from_env) = CubeConfig::from_env_with(env) {
            if let Ok(variables) = current_variables(&from_env, "default", env) {
                candidates.push(("default".to_string(), own_settings("default", variables)));
            }
        }
    }
    for (name, variables) in &candidates {
        save(store, key, name, variables)
            .await
            .map_err(|e| format!("cannot import data source {name}: {e}"))?;
        report.imported.push(name.clone());
    }

    // The files no longer declare what RBAC now holds: everything they
    // declare is in RBAC by now. Done on every start, so a strip an earlier
    // start could not finish is finished.
    let declared: Vec<String> = cube_config.data_sources.keys().cloned().collect();
    if let Some(file) = config_file.filter(|_| !declared.is_empty()) {
        strip_files(file, cube_config, &declared)?;
    }

    let mut loaded = load_all(store, key).await?;
    // `default` first, as the file usually lists it.
    loaded.sort_by_key(|ds| (ds.name != "default", ds.name.clone()));
    let data_sources = &mut cube_config.data_sources;
    data_sources.clear();
    for ds in &loaded {
        let resolved = data_source_from_variables(&ds.name, &ds.variables)
            .map_err(|e| format!("data source {}: {e}", ds.name))?;
        data_sources.insert(ds.name.clone(), resolved);
        for (var, value) in &ds.variables {
            report
                .env
                .insert(key_by_data_source(var, &ds.name), value.clone());
        }
    }
    report.loaded = data_sources.len();
    Ok(report)
}

/// What of `variables` belongs to data source `name` alone: for `default`,
/// whose variables are the global `CUBEJS_DB_*`, only the settings the Data
/// Source page manages (not tuning knobs like `CUBEJS_DB_QUERY_TIMEOUT`).
fn own_settings(name: &str, mut variables: BTreeMap<String, String>) -> BTreeMap<String, String> {
    if name == "default" {
        variables.retain(|var, _| MANAGED_FIELDS.contains(&var.as_str()));
    }
    variables
}

/// Backs up `cube.yml` and `.env`, then takes `names` out of both: their
/// blocks from `cube.yml`, their own `CUBEJS_DS_<NAME>_*` lines from `.env`.
/// Variables a block only refers to (`{ env: SOFIADB_HOST }`, `default`'s
/// `CUBEJS_DB_*`) stay: the start-up scripts may read them.
fn strip_files(file: &Path, cube_config: &CubeConfig, names: &[String]) -> Result<(), String> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let env_file = file.with_file_name(".env");
    let sources: Vec<&str> = cube_config.data_sources.keys().map(String::as_str).collect();
    let env_lines = |name: &str| -> Vec<String> {
        if name == "default" {
            return Vec::new();
        }
        env_keys(&env_file)
            .into_iter()
            .filter(|k| own_setting(k, name, &sources).is_some())
            .collect()
    };
    let backup = |path: &Path| -> Result<(), String> {
        if path.exists() {
            let mut name = path.file_name().unwrap_or_default().to_os_string();
            name.push(format!(".bak-{stamp}"));
            std::fs::copy(path, path.with_file_name(name))
                .map_err(|e| format!("cannot back up {}: {e}", path.display()))?;
        }
        Ok(())
    };

    let original = std::fs::read_to_string(file)
        .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    let mut text = original.clone();
    for name in names {
        match remove_from_config(&text, name) {
            Some(updated) => text = updated,
            None => eprintln!(
                "⚠️  data source {name} is in RBAC but its block in {} could not be taken out; remove it by hand",
                file.display()
            ),
        }
    }
    let env_changes: Vec<(&String, Vec<String>)> = names
        .iter()
        .map(|name| (name, env_lines(name)))
        .filter(|(_, lines)| !lines.is_empty())
        .collect();
    if text == original && env_changes.is_empty() {
        return Ok(());
    }

    backup(file)?;
    backup(&env_file)?;
    if text != original {
        write_atomically(file, &text).map_err(|e| e.body.error)?;
    }
    for (name, remove) in env_changes {
        update_env(&env_file, &[], &remove, &header(name), true).map_err(|e| e.body.error)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> ImportReport {
        ImportReport {
            imported: vec![],
            loaded: 2,
            env: [
                ("CUBEJS_DB_PASS".to_string(), "pg-secret".to_string()),
                ("CUBEJS_DS_EXTRA_DB_NAME".to_string(), "/tmp/x.db".to_string()),
                ("CUBEJS_DS_EXTRA_DB_SSL_CA".to_string(), "ca".to_string()),
            ]
            .into(),
        }
    }

    #[test]
    fn debug_shows_no_value() {
        let shown = format!("{:?}", report());
        assert!(shown.contains("CUBEJS_DB_PASS") && !shown.contains("pg-secret"), "{shown}");
    }

    #[test]
    fn superseded_env_is_what_rbac_holds_or_manages_and_the_process_has() {
        let process = [
            "CUBEJS_DB_PASS",
            "CUBEJS_DB_USER",
            "CUBEJS_DS_EXTRA_DB_PASS",
            "CUBEJS_DS_EXTRA_DB_SSL_CA",
            "CUBEJS_DB_QUERY_TIMEOUT",
            "CUBEJS_DS_GONE_DB_PASS",
            "SOFIADB_PASSWORD",
        ];
        let names = vec!["default".to_string(), "extra".to_string()];
        let keys = superseded_env(&report(), &names, |k| process.contains(&k));
        assert_eq!(
            keys,
            [
                "CUBEJS_DB_PASS",
                "CUBEJS_DB_USER",
                "CUBEJS_DS_EXTRA_DB_PASS",
                "CUBEJS_DS_EXTRA_DB_SSL_CA",
            ]
        );
    }
}
