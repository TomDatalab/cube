//! Data source settings kept in the RBAC database: the `CUBEJS_DB_*`
//! variables the Data Source page edits, the secret ones sealed with
//! [`SecretKey`]. With RBAC on they replace `cube.yml` and `.env`.

use std::collections::BTreeMap;

use uuid::Uuid;

use super::secrets::SecretKey;
use super::store::{DataSourceRow, Store};
use crate::handlers::data_sources::kept_if_empty;

/// A data source with its settings in the clear, `CUBEJS_DB_TYPE` included.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredDataSource {
    pub id: Uuid,
    pub name: String,
    pub variables: BTreeMap<String, String>,
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

pub async fn load_all(store: &Store, key: &SecretKey) -> Result<Vec<StoredDataSource>, String> {
    let rows = store.data_sources().await.map_err(|e| e.to_string())?;
    rows.into_iter().map(|row| opened(row, key)).collect()
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
    let db_type = variables
        .get(TYPE)
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| format!("{name}: no {TYPE}"))?;
    let (plain, secret) = split(variables);
    let sealed = if secret.is_empty() {
        None
    } else {
        let json = serde_json::to_vec(&secret).map_err(|e| e.to_string())?;
        Some(key.seal(&json))
    };
    store
        .put_data_source(name, db_type, &plain, sealed.as_deref())
        .await
        .map_err(|e| e.to_string())
}

const TYPE: &str = "CUBEJS_DB_TYPE";

fn opened(row: DataSourceRow, key: &SecretKey) -> Result<StoredDataSource, String> {
    let mut variables = row.settings;
    if let Some(sealed) = &row.secrets {
        let json = key
            .open(sealed)
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
