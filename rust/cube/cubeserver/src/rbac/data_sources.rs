//! Data source settings kept in the RBAC database: the `CUBEJS_DB_*`
//! variables the Data Source page edits, the secret ones sealed with
//! [`SecretKey`]. With RBAC on they replace `cube.yml` and `.env`.

use std::collections::BTreeMap;

use uuid::Uuid;

use super::secrets::SecretKey;
use super::store::{DataSourceRow, Store};
use crate::handlers::data_sources::kept_if_empty;

/// A data source with its settings in the clear, `CUBEJS_DB_TYPE` included.
#[derive(Clone, PartialEq)]
pub struct StoredDataSource {
    pub id: Uuid,
    pub name: String,
    pub variables: BTreeMap<String, String>,
}

/// The variables' names only: their values (a password) never reach a log.
impl std::fmt::Debug for StoredDataSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredDataSource")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("variables", &self.variables.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// A stored data source whose settings cannot be read: its secrets were
/// sealed under another key, or the row is damaged.
#[derive(Debug, Clone, PartialEq)]
pub struct Unreadable {
    pub id: Uuid,
    pub name: String,
    pub db_type: String,
    pub error: String,
}

/// `variables` as (plain, secret), both without `CUBEJS_DB_TYPE`: secret is
/// what the edit form never sends back (a password, a key, a URL carrying a
/// password).
pub fn split(
    variables: &BTreeMap<String, String>,
) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let mut plain = BTreeMap::new();
    let mut secret = BTreeMap::new();
    for (var, value) in variables {
        if var == TYPE {
            continue;
        }
        if kept_if_empty(var, variables) {
            secret.insert(var.clone(), value.clone());
        } else {
            plain.insert(var.clone(), value.clone());
        }
    }
    (plain, secret)
}

/// Every stored data source; an error when one cannot be read.
pub async fn load_all(store: &Store, key: &SecretKey) -> Result<Vec<StoredDataSource>, String> {
    let rows = store.data_sources().await.map_err(|e| e.to_string())?;
    rows.into_iter().map(|row| opened(row, key)).collect()
}

/// Every stored data source, each read on its own: one that cannot be read
/// is an [`Unreadable`] beside the others. An error only when the database
/// cannot be.
pub async fn load_each(
    store: &Store,
    key: &SecretKey,
) -> Result<Vec<Result<StoredDataSource, Unreadable>>, String> {
    let rows = store.data_sources().await.map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let (id, name, db_type) = (row.id, row.name.clone(), row.db_type.clone());
            opened(row, key).map_err(|error| Unreadable { id, name, db_type, error })
        })
        .collect())
}

/// Seals again, bound to their row, the secrets an earlier release sealed
/// without associated data. Returns the data sources resealed; one that
/// opens neither way is left for [`load_all`] to report.
pub async fn reseal_unbound(store: &Store, key: &SecretKey) -> Result<Vec<String>, String> {
    let mut resealed = Vec::new();
    for row in store.data_sources().await.map_err(|e| e.to_string())? {
        let Some(sealed) = &row.secrets else { continue };
        if key.open(sealed, &aad(&row.name)).is_ok() {
            continue;
        }
        let Ok(plain) = key.open(sealed, b"") else { continue };
        store
            .put_data_source(&row.name, &row.db_type, &row.settings, Some(&key.seal(&plain, &aad(&row.name))))
            .await
            .map_err(|e| e.to_string())?;
        resealed.push(row.name);
    }
    Ok(resealed)
}

/// What a data source's secrets are bound to: its row, by name.
fn aad(name: &str) -> Vec<u8> {
    format!("cube_app.data_sources:{name}").into_bytes()
}

pub async fn load(
    store: &Store,
    key: &SecretKey,
    name: &str,
) -> Result<Option<StoredDataSource>, String> {
    match store.data_source(name).await.map_err(|e| e.to_string())? {
        Some(row) => opened(row, key).map(Some),
        None => Ok(None),
    }
}

/// Adds or replaces data source `name` with exactly `variables`, which
/// must name its `CUBEJS_DB_TYPE`. Returns its id.
pub async fn save(
    store: &Store,
    key: &SecretKey,
    name: &str,
    variables: &BTreeMap<String, String>,
) -> Result<Uuid, String> {
    let row = sealed(key, name, variables)?;
    store
        .put_data_source(name, &row.db_type, &row.settings, row.secrets.as_deref())
        .await
        .map_err(|e| e.to_string())
}

/// `variables` as the row [`save`] writes (its `id` is not set yet): for a
/// write inside a transaction of the caller's.
pub fn sealed(
    key: &SecretKey,
    name: &str,
    variables: &BTreeMap<String, String>,
) -> Result<DataSourceRow, String> {
    let db_type = variables
        .get(TYPE)
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| format!("{name}: no {TYPE}"))?;
    let (plain, secret) = split(variables);
    let sealed = if secret.is_empty() {
        None
    } else {
        let json = serde_json::to_vec(&secret).map_err(|e| e.to_string())?;
        Some(key.seal(&json, &aad(name)))
    };
    Ok(DataSourceRow {
        id: Uuid::nil(),
        name: name.to_string(),
        db_type: db_type.clone(),
        settings: plain,
        secrets: sealed,
        damaged: None,
    })
}

const TYPE: &str = "CUBEJS_DB_TYPE";

fn opened(row: DataSourceRow, key: &SecretKey) -> Result<StoredDataSource, String> {
    if let Some(damaged) = &row.damaged {
        return Err(format!("data source {}: {damaged}", row.name));
    }
    let mut variables = row.settings;
    if let Some(sealed) = &row.secrets {
        let json = key
            .open(sealed, &aad(&row.name))
            .map_err(|e| format!("data source {}: {e}", row.name))?;
        let secret: BTreeMap<String, String> = serde_json::from_slice(&json)
            .map_err(|e| format!("data source {}: {e}", row.name))?;
        variables.extend(secret);
    }
    variables.insert(TYPE.to_string(), row.db_type);
    Ok(StoredDataSource {
        id: row.id,
        name: row.name,
        variables,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_shows_no_value() {
        let ds = StoredDataSource {
            id: Uuid::nil(),
            name: "sales".into(),
            variables: [("CUBEJS_DB_PASS".to_string(), "hunter2-secret".to_string())].into(),
        };
        let shown = format!("{ds:?}");
        assert!(shown.contains("CUBEJS_DB_PASS") && !shown.contains("hunter2-secret"), "{shown}");
    }

    #[test]
    fn split_puts_passwords_and_urls_with_passwords_aside() {
        let vars: BTreeMap<String, String> = [
            ("CUBEJS_DB_TYPE", "postgres"),
            ("CUBEJS_DB_HOST", "h"),
            ("CUBEJS_DB_PASS", "p"),
            ("CUBEJS_DB_URL", "postgres://u:p@h/db"),
            ("CUBEJS_DB_SCHEMA", "public"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let (plain, secret) = split(&vars);
        assert_eq!(
            plain.keys().collect::<Vec<_>>(),
            ["CUBEJS_DB_HOST", "CUBEJS_DB_SCHEMA"]
        );
        assert_eq!(
            secret.keys().collect::<Vec<_>>(),
            ["CUBEJS_DB_PASS", "CUBEJS_DB_URL"]
        );
    }
}
