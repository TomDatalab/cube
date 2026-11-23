//! What the SofiaDB sync needs to know about the model before it generates:
//! the names already taken and the tables hand-written cubes already cover.
//! The same rule as `hand_written()` in `sync-sofiadb-model.py`.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde::{Deserialize, Serialize};

/// The `managed_by` of the rows the sync writes.
pub const SYNC_MANAGER: &str = "sofiadb-sync";

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncContext {
    /// Every cube and view name.
    pub taken: BTreeSet<String>,
    /// `db.table` -> the hand-written cube that reads it.
    pub covered: BTreeMap<String, String>,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The first `FROM <ref>` of a query: `\bfrom\s+([\w.`"]+)`, case-insensitive.
fn from_ref(sql: &str) -> Option<String> {
    let lower = sql.to_lowercase();
    // Lowercasing can change byte offsets; fall back to the chars of `sql`.
    let chars: Vec<char> = if lower.chars().count() == sql.chars().count() {
        lower.chars().collect()
    } else {
        sql.chars().collect()
    };
    let n = chars.len();
    let mut i = 0;
    while i + 4 <= n {
        if chars[i..i + 4] == ['f', 'r', 'o', 'm'] && (i == 0 || !is_word(chars[i - 1])) {
            let mut j = i + 4;
            let spaces = j;
            while j < n && chars[j].is_whitespace() {
                j += 1;
            }
            if j > spaces {
                let start = j;
                while j < n && (is_word(chars[j]) || matches!(chars[j], '.' | '`' | '"')) {
                    j += 1;
                }
                if j > start {
                    return Some(chars[start..j].iter().collect());
                }
            }
        }
        i += 1;
    }
    None
}

/// `sources` are `(source path, content)`; `managed` holds the source paths
/// of the sync's own rows, which do not count.
pub fn sync_context(
    sources: &[(String, String)],
    managed: &HashSet<String>,
    connection: &str,
    default_db: &str,
) -> SyncContext {
    let mut out = SyncContext::default();
    for (path, content) in sources {
        let lower = path.to_ascii_lowercase();
        if managed.contains(path) || !(lower.ends_with(".yml") || lower.ends_with(".yaml")) {
            continue;
        }
        let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(content) else {
            continue;
        };
        for kind in ["cubes", "views"] {
            let Some(items) = doc.get(kind).and_then(|v| v.as_sequence()) else {
                continue;
            };
            for item in items {
                let Some(name) = item.get("name").and_then(|v| v.as_str()) else {
                    continue;
                };
                out.taken.insert(name.to_string());
                let text = |key: &str| item.get(key).and_then(|v| v.as_str()).unwrap_or("");
                if kind != "cubes" || text("data_source") != connection {
                    continue;
                }
                let mut reference = text("sql_table").to_string();
                if reference.is_empty() {
                    reference = from_ref(text("sql")).unwrap_or_default();
                }
                let reference = reference.replace(['`', '"'], "");
                if reference.is_empty() {
                    continue;
                }
                let (db, table) = match reference.rsplit_once('.') {
                    Some((db, table)) => (db, table),
                    None => ("", reference.as_str()),
                };
                let db = if db.is_empty() { default_db } else { db };
                out.covered.insert(format!("{db}.{table}"), name.to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(path: &str, content: &str) -> (String, String) {
        (path.to_string(), content.to_string())
    }

    #[test]
    fn context_names_and_covered() {
        let sources = vec![
            src(
                "org/eventos.yml",
                "cubes:\n  - name: eventos\n    sql_table: cluster_test.eventos_local\n    data_source: sofiadb\nviews:\n  - name: visao\n",
            ),
            src(
                "org/sql.yml",
                "cubes:\n  - name: vendas\n    data_source: sofiadb\n    sql: \"SELECT 1 FROM `pedidos` p\"\n  - name: outra\n    data_source: default\n    sql_table: x.y\n",
            ),
        ];
        let c = sync_context(&sources, &HashSet::new(), "sofiadb", "dfos");
        assert_eq!(c.covered["cluster_test.eventos_local"], "eventos");
        assert_eq!(c.covered["dfos.pedidos"], "vendas");
        assert_eq!(c.covered.len(), 2);
        for n in ["eventos", "visao", "vendas", "outra"] {
            assert!(c.taken.contains(n), "{n}");
        }
    }

    #[test]
    fn managed_sources_do_not_count() {
        let sources = vec![
            src(
                "org/sofiadb/a.yml",
                "cubes:\n  - name: gerado\n    sql_table: db.t\n    data_source: sofiadb\n",
            ),
            src("org/b.yml", "cubes:\n  - name: meu\n    sql_table: db.u\n    data_source: sofiadb\n"),
        ];
        let managed: HashSet<String> = ["org/sofiadb/a.yml".to_string()].into();
        let c = sync_context(&sources, &managed, "sofiadb", "db");
        assert!(!c.taken.contains("gerado"));
        assert_eq!(c.covered.keys().collect::<Vec<_>>(), ["db.u"]);
    }
}
