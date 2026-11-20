//! With RBAC on, the data sources come from its database: at start-up,
//! before anything reads them, the ones `cube.yml` still declares are
//! imported once (and taken out of `cube.yml` and `.env`, which are backed
//! up first), then every stored one replaces the file's.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use cubeconfig::data_source::key_by_data_source;
use cubeconfig::{data_source_from_variables, CubeConfig, Env};

use super::data_sources::{load_all, save};
use super::secrets::SecretKey;
use super::store::Store;
use crate::handlers::data_sources::{
    current_variables, env_keys, env_with_file, header, own_setting, remove_from_config, update_env,
    write_atomically,
};

#[derive(Debug, Default)]
pub struct ImportReport {
    /// Data sources copied from the files this time.
    pub imported: Vec<String>,
    /// Data sources loaded from RBAC.
    pub loaded: usize,
    /// Every stored setting as the variable a driver reads
    /// (`CUBEJS_DS_<NAME>_*`, `CUBEJS_DB_*` for `default`).
    pub env: HashMap<String, String>,
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
            candidates.push((name.clone(), variables));
        }
    }
    if !stored.contains("default") && !cube_config.data_sources.contains_key("default") {
        if let Ok(from_env) = CubeConfig::from_env_with(env) {
            if let Ok(variables) = current_variables(&from_env, "default", env) {
                candidates.push(("default".to_string(), variables));
            }
        }
    }
    for (name, variables) in &candidates {
        save(store, key, name, variables)
            .await
            .map_err(|e| format!("cannot import data source {name}: {e}"))?;
        report.imported.push(name.clone());
    }

    // The files no longer declare what RBAC now holds.
    if let Some(file) = config_file.filter(|_| !report.imported.is_empty()) {
        strip_files(file, cube_config, &report.imported)?;
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

/// Backs up `cube.yml` and `.env`, then takes `names` out of both: their
/// blocks from `cube.yml`, their own `CUBEJS_DS_<NAME>_*` lines from `.env`.
/// Variables a block only refers to (`{ env: SOFIADB_HOST }`, `default`'s
/// `CUBEJS_DB_*`) stay: the start-up scripts may read them.
fn strip_files(file: &Path, cube_config: &CubeConfig, names: &[String]) -> Result<(), String> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let env_file = file.with_file_name(".env");
    let backup = |path: &Path| -> Result<(), String> {
        if path.exists() {
            let mut name = path.file_name().unwrap_or_default().to_os_string();
            name.push(format!(".bak-{stamp}"));
            std::fs::copy(path, path.with_file_name(name))
                .map_err(|e| format!("cannot back up {}: {e}", path.display()))?;
        }
        Ok(())
    };
    backup(file)?;
    backup(&env_file)?;

    let mut text = std::fs::read_to_string(file)
        .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    for name in names {
        if let Some(updated) = remove_from_config(&text, name) {
            text = updated;
        }
    }
    write_atomically(file, &text).map_err(|e| e.body.error)?;

    let sources: Vec<&str> = cube_config.data_sources.keys().map(String::as_str).collect();
    for name in names.iter().filter(|n| n.as_str() != "default") {
        let remove: Vec<String> = env_keys(&env_file)
            .into_iter()
            .filter(|k| own_setting(k, name, &sources).is_some())
            .collect();
        if !remove.is_empty() {
            update_env(&env_file, &[], &remove, &header(name), true).map_err(|e| e.body.error)?;
        }
    }
    Ok(())
}
