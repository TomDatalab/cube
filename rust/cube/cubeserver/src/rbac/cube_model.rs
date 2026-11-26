//! Models: tables on a canvas, linked by their columns, built into ONE cube
//! whose `sql` JOINs them. Pure: [`validate`] checks the shape, [`build`]
//! checks it against the connection's columns and writes the cube YAML.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value as Yaml};
use uuid::Uuid;

use crate::handlers::scaffold::{cube_type, quote, snake};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Definition {
    /// 2 for a definition the Redgate-style editor wrote; none is the first
    /// format, read the same way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format_version: Option<u32>,
    pub data_source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub tables: Vec<ModelTable>,
    pub primary: Option<String>,
    pub primary_key: Option<String>,
    pub joins: Vec<ModelJoin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<Settings>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<Note>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub areas: Vec<Area>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub views: Vec<ModelView>,
}

/// Free text on the canvas.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Note {
    pub id: String,
    pub x: f64,
    pub y: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z: Option<i32>,
    pub text: String,
    /// A palette name from [`COLORS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// A labelled rectangle drawn behind the other objects.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Area {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub border: Option<String>,
}

/// A view designed on the canvas; its query is free text.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelView {
    pub id: String,
    pub name: String,
    pub schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    pub sql: String,
    pub columns: Vec<ViewColumn>,
    /// Table aliases the view reads.
    pub depends_on_tables: Vec<String>,
    /// Ids of the views it reads.
    pub depends_on_views: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script_after: Option<String>,
    pub x: f64,
    pub y: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z: Option<i32>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
}

/// The model's own options, as Redgate's model properties.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Settings {
    /// Validation rules switched on or off, by id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<std::collections::BTreeMap<String, bool>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<FormatSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql_generation: Option<SqlGeneration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scripts: Option<ModelScripts>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FormatSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_types: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_nullable: Option<bool>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SqlGeneration {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drop: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comments: Option<bool>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelScripts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub create_after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drop_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drop_after: Option<String>,
}

/// A table on the canvas. The same table dragged twice gets two aliases.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelTable {
    pub alias: String,
    pub schema: String,
    pub table: String,
    pub x: f64,
    pub y: f64,
    /// A palette name from [`COLORS`]; none means the neutral.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// The design of a table not in the database yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<DraftTable>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// A size fixed on the canvas; none follows the content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// The same table shown again elsewhere on the canvas.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shortcuts: Vec<Shortcut>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Shortcut {
    pub id: String,
    pub x: f64,
    pub y: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// A table being designed: it becomes real with [`create_statements`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DraftTable {
    pub columns: Vec<DraftColumn>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_key_name: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unique_keys: Vec<UniqueKey>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub indexes: Vec<TableIndex>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<TableCheck>,
    /// SQL run before and after the table's `CREATE TABLE`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script_before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script_after: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UniqueKey {
    pub name: String,
    pub columns: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TableIndex {
    pub name: String,
    pub columns: Vec<IndexColumn>,
    #[serde(default)]
    pub unique: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IndexColumn {
    pub name: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub desc: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TableCheck {
    pub name: String,
    pub expression: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DraftColumn {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub nullable: bool,
    #[serde(default)]
    pub primary_key: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// SQL text, as Redgate takes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_increment: bool,
    /// MySQL only.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unsigned: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub zerofill: bool,
}

pub const COLORS: [&str; 8] = [
    "slate", "blue", "green", "amber", "red", "violet", "pink", "teal",
];
pub const TYPES: [&str; 7] = [
    "text",
    "integer",
    "bigint",
    "numeric",
    "boolean",
    "date",
    "timestamp",
];

/// `^[A-Za-z_][A-Za-z0-9_]*$`, at most 63 characters.
fn valid_ident(s: &str) -> bool {
    let mut chars = s.chars();
    s.len() <= 63
        && chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn validate_draft(table: &ModelTable, draft: &DraftTable) -> Result<(), String> {
    let name = format!("{}.{}", table.schema, table.table);
    if !valid_ident(&table.schema) {
        return Err(format!("Schema inválido: {}", table.schema));
    }
    if !valid_ident(&table.table) {
        return Err(format!("Nome de tabela inválido: {}", table.table));
    }
    if draft.columns.is_empty() {
        return Err(format!("A tabela {name} precisa de ao menos uma coluna"));
    }
    let mut seen = HashSet::new();
    for column in &draft.columns {
        if !valid_ident(&column.name) {
            return Err(format!("Nome de coluna inválido: {}", column.name));
        }
        if !valid_type(Some("postgres"), &column.kind) && !valid_type(Some("mysql"), &column.kind) {
            return Err(format!("Tipo inválido: {}", column.kind));
        }
        if !seen.insert(column.name.to_ascii_lowercase()) {
            return Err(format!("A coluna {} se repete em {name}", column.name));
        }
    }
    let has = |column: &str| draft.columns.iter().any(|c| c.name == column);
    let mut names = HashSet::new();
    let mut named = |object: &str| -> Result<(), String> {
        if !valid_ident(object) {
            return Err(format!("Nome inválido: {object}"));
        }
        if !names.insert(object.to_ascii_lowercase()) {
            return Err(format!("O nome {object} se repete em {name}"));
        }
        Ok(())
    };
    if let Some(pk) = draft.primary_key_name.as_deref().filter(|n| !n.is_empty()) {
        named(pk)?;
    }
    for key in &draft.unique_keys {
        named(&key.name)?;
        if key.columns.is_empty() {
            return Err(format!("A chave única {} precisa de colunas", key.name));
        }
        if let Some(c) = key.columns.iter().find(|c| !has(c)) {
            return Err(format!(
                "A chave única {} usa a coluna {c}, que não existe",
                key.name
            ));
        }
    }
    for index in &draft.indexes {
        named(&index.name)?;
        if let Some(c) = index.columns.iter().find(|c| !has(&c.name)) {
            return Err(format!(
                "O índice {} usa a coluna {}, que não existe",
                index.name, c.name
            ));
        }
    }
    for check in &draft.checks {
        named(&check.name)?;
    }
    Ok(())
}

/// `postgres` or `mysql` for a connection type the model designs for.
pub fn dialect(db_type: &str) -> Option<&'static str> {
    match db_type {
        "postgres" => Some("postgres"),
        "mysql" => Some("mysql"),
        _ => None,
    }
}

/// What a type takes in parentheses: nothing, a size, or a precision with
/// an optional scale; `required` when it cannot go without.
#[derive(Clone, Copy)]
enum Param {
    None,
    Size { required: bool },
    Precision,
}

const POSTGRES_TYPES: &[(&str, Param)] = &[
    ("smallint", Param::None),
    ("integer", Param::None),
    ("bigint", Param::None),
    ("serial", Param::None),
    ("bigserial", Param::None),
    ("numeric", Param::Precision),
    ("decimal", Param::Precision),
    ("real", Param::None),
    ("double precision", Param::None),
    ("boolean", Param::None),
    ("char", Param::Size { required: false }),
    ("varchar", Param::Size { required: false }),
    ("text", Param::None),
    ("date", Param::None),
    ("time", Param::None),
    ("timestamp", Param::None),
    ("timestamptz", Param::None),
    ("interval", Param::None),
    ("uuid", Param::None),
    ("json", Param::None),
    ("jsonb", Param::None),
    ("bytea", Param::None),
];

const MYSQL_TYPES: &[(&str, Param)] = &[
    ("tinyint", Param::Size { required: false }),
    ("smallint", Param::Size { required: false }),
    ("mediumint", Param::Size { required: false }),
    ("int", Param::Size { required: false }),
    ("bigint", Param::Size { required: false }),
    ("decimal", Param::Precision),
    ("float", Param::None),
    ("double", Param::None),
    ("boolean", Param::None),
    ("bit", Param::Size { required: false }),
    ("char", Param::Size { required: false }),
    ("varchar", Param::Size { required: true }),
    ("tinytext", Param::None),
    ("text", Param::None),
    ("mediumtext", Param::None),
    ("longtext", Param::None),
    ("date", Param::None),
    ("time", Param::None),
    ("datetime", Param::None),
    ("timestamp", Param::None),
    ("year", Param::None),
    ("json", Param::None),
    ("blob", Param::None),
    ("binary", Param::Size { required: false }),
    ("varbinary", Param::Size { required: true }),
];

/// A type of the dialect's closed list, its size or precision checked; the
/// 7 portable [`TYPES`] are valid everywhere (models before dialects).
pub fn valid_type(dialect: Option<&str>, ty: &str) -> bool {
    let t = ty.trim().to_ascii_lowercase();
    if TYPES.contains(&t.as_str()) {
        return true;
    }
    let list = match dialect {
        Some("postgres") => POSTGRES_TYPES,
        Some("mysql") => MYSQL_TYPES,
        _ => return false,
    };
    let (base, params) = match t.find('(') {
        Some(at) => {
            let Some(inner) = t[at + 1..].strip_suffix(')') else {
                return false;
            };
            (t[..at].trim(), Some(inner))
        }
        None => (t.as_str(), None),
    };
    let Some((_, param)) = list.iter().find(|(name, _)| *name == base) else {
        return false;
    };
    // Each number of the parentheses; a part that is not one fails.
    let numbers: Option<Option<Vec<u32>>> = params.map(|p| {
        p.split(',')
            .map(|n| n.trim().parse::<u32>().ok())
            .collect()
    });
    match (param, numbers) {
        (Param::None, None) => true,
        (Param::Size { required }, None) => !required,
        (Param::Precision, None) => true,
        (_, Some(None)) | (Param::None, Some(_)) => false,
        (Param::Size { .. }, Some(Some(n))) => n.len() == 1 && n[0] >= 1,
        (Param::Precision, Some(Some(n))) => match n.as_slice() {
            [p] => *p >= 1,
            [p, s] => *p >= 1 && s <= p,
            _ => false,
        },
    }
}

/// Every draft column's type in the connection's dialect, or among the
/// portable types for a connection with none.
pub fn validate_types(def: &Definition, db_type: &str) -> Result<(), String> {
    let d = dialect(db_type);
    for table in &def.tables {
        let Some(draft) = &table.draft else { continue };
        for column in &draft.columns {
            if !valid_type(d, &column.kind) {
                return Err(format!(
                    "Tipo inválido para {}.{}.{}: {}",
                    table.schema, table.table, column.name, column.kind
                ));
            }
        }
    }
    for view in &def.views {
        for column in &view.columns {
            if !valid_type(d, &column.kind) {
                return Err(format!(
                    "Tipo inválido para {}.{}: {}",
                    view.name, column.name, column.kind
                ));
            }
        }
    }
    Ok(())
}

/// What "Create in database" runs for the draft tables: see
/// [`super::ddl::create_statements`].
pub fn create_statements(
    def: &Definition,
    db_type: &str,
) -> Result<Vec<super::ddl::Statement>, String> {
    super::ddl::create_statements(def, db_type)
}

/// A draft's shape checked before its DDL is written.
pub(crate) fn validate_draft_for_ddl(table: &ModelTable, draft: &DraftTable) -> Result<(), String> {
    validate_draft(table, draft)
}

/// A link between two columns: `relationship` is one_to_one, one_to_many or
/// many_to_one, read from `from` to `to`; `join` is left or inner.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelJoin {
    pub from: End,
    pub to: End,
    pub relationship: String,
    pub join: String,
    /// Bends of the line, in canvas coordinates.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<Point>,
    /// The line leaves its tables on the other side.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub swapped: bool,
    /// Not drawn on the canvas; still a link.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
    /// What the reference is as a foreign key, as Redgate shows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Of the child side: `0..*`, `0..1`, `1..*` or `1..1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cardinality: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mandatory: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_update: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_delete: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_role: Option<String>,
    /// More column pairs after `from.column` = `to.column`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pairs: Vec<ColumnPair>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ColumnPair {
    pub from: String,
    pub to: String,
}

pub const CARDINALITIES: [&str; 4] = ["0..*", "0..1", "1..*", "1..1"];
pub const ACTIONS: [&str; 5] = ["none", "restrict", "cascade", "set null", "set default"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct End {
    pub alias: String,
    pub column: String,
}

const RELATIONSHIPS: [&str; 3] = ["one_to_one", "one_to_many", "many_to_one"];
const JOINS: [&str; 2] = ["left", "inner"];

/// The shape alone, valid while drafting: unique non-empty aliases, a
/// `primary` among them, links between known aliases with known values, and
/// no second path between two tables. A table not linked yet is fine here
/// (the editor saves while one is being placed); [`build`] refuses it. A
/// model with no tables is a valid draft.
pub fn validate(def: &Definition) -> Result<(), String> {
    if let Some(v) = def.format_version {
        if v != 2 {
            return Err(format!("formatVersion inválido: {v}"));
        }
    }
    let mut aliases = HashSet::new();
    let mut drafts = HashSet::new();
    let mut shortcuts = HashSet::new();
    for table in &def.tables {
        for size in [table.width, table.height].into_iter().flatten() {
            if !size.is_finite() || size <= 0.0 {
                return Err(format!("Tamanho inválido em {}: {size}", table.alias));
            }
        }
        for s in &table.shortcuts {
            if !s.x.is_finite() || !s.y.is_finite() {
                return Err(format!("Ponto inválido no atalho {}", s.id));
            }
            if !shortcuts.insert(s.id.as_str()) {
                return Err(format!("O atalho {} se repete", s.id));
            }
        }
        if table.alias.trim().is_empty() {
            return Err("Toda tabela precisa de um alias".into());
        }
        if !aliases.insert(table.alias.as_str()) {
            return Err(format!("O alias {} se repete", table.alias));
        }
        if let Some(color) = &table.color {
            if !COLORS.contains(&color.as_str()) {
                return Err(format!("Cor inválida: {color}"));
            }
        }
        if let Some(draft) = &table.draft {
            validate_draft(table, draft)?;
            if !drafts.insert((table.schema.as_str(), table.table.as_str())) {
                return Err(format!(
                    "A tabela nova {}.{} se repete",
                    table.schema, table.table
                ));
            }
        }
    }
    if let Some(primary) = &def.primary {
        if !aliases.contains(primary.as_str()) {
            return Err(format!("A tabela principal {primary} não está no model"));
        }
    }

    // Union-find: a link between two tables already connected closes a cycle.
    let mut parent: HashMap<&str, &str> = aliases.iter().map(|a| (*a, *a)).collect();
    fn root<'a>(parent: &HashMap<&'a str, &'a str>, mut a: &'a str) -> &'a str {
        while parent[a] != a {
            a = parent[a];
        }
        a
    }
    let mut ids = HashSet::new();
    for join in &def.joins {
        if let Some(id) = &join.id {
            if !ids.insert(id.as_str()) {
                return Err(format!("A ligação {id} se repete"));
            }
        }
        if let Some(name) = join.name.as_deref().filter(|n| !n.is_empty()) {
            if !valid_ident(name) {
                return Err(format!("Nome inválido: {name}"));
            }
        }
        if let Some(c) = &join.cardinality {
            if !CARDINALITIES.contains(&c.as_str()) {
                return Err(format!("Cardinalidade inválida: {c}"));
            }
        }
        for action in [&join.on_update, &join.on_delete].into_iter().flatten() {
            if !ACTIONS.contains(&action.as_str()) {
                return Err(format!("Ação inválida: {action}"));
            }
        }
        for end in [&join.from, &join.to] {
            if !aliases.contains(end.alias.as_str()) {
                return Err(format!(
                    "A ligação usa a tabela {}, que não está no model",
                    end.alias
                ));
            }
        }
        if !RELATIONSHIPS.contains(&join.relationship.as_str()) {
            return Err(format!("Relacionamento inválido: {}", join.relationship));
        }
        if !JOINS.contains(&join.join.as_str()) {
            return Err(format!("JOIN inválido: {}", join.join));
        }
        if join
            .points
            .iter()
            .any(|p| !p.x.is_finite() || !p.y.is_finite())
        {
            return Err(format!(
                "Ponto inválido na ligação {} → {}",
                join.from.alias, join.to.alias
            ));
        }
        let (a, b) = (
            root(&parent, &join.from.alias),
            root(&parent, &join.to.alias),
        );
        if a == b {
            return Err(format!(
                "{} e {} já estão ligadas",
                join.from.alias, join.to.alias
            ));
        }
        parent.insert(a, b);
    }

    let size_ok = |id: &str, sizes: &[Option<f64>]| -> Result<(), String> {
        for size in sizes.iter().flatten() {
            if !size.is_finite() || *size <= 0.0 {
                return Err(format!("Tamanho inválido em {id}: {size}"));
            }
        }
        Ok(())
    };
    let color_ok = |color: &Option<String>| -> Result<(), String> {
        match color {
            Some(c) if !COLORS.contains(&c.as_str()) => Err(format!("Cor inválida: {c}")),
            _ => Ok(()),
        }
    };
    let mut seen = HashSet::new();
    for note in &def.notes {
        if !seen.insert(note.id.as_str()) {
            return Err(format!("O objeto {} se repete", note.id));
        }
        size_ok(&note.id, &[note.width, note.height])?;
        color_ok(&note.color)?;
    }
    let mut seen = HashSet::new();
    for area in &def.areas {
        if !seen.insert(area.id.as_str()) {
            return Err(format!("O objeto {} se repete", area.id));
        }
        size_ok(&area.id, &[Some(area.width), Some(area.height)])?;
        color_ok(&area.fill)?;
        color_ok(&area.border)?;
    }
    let view_ids: HashSet<&str> = def.views.iter().map(|v| v.id.as_str()).collect();
    let mut seen = HashSet::new();
    let mut names = HashSet::new();
    for view in &def.views {
        if !seen.insert(view.id.as_str()) {
            return Err(format!("O objeto {} se repete", view.id));
        }
        size_ok(&view.id, &[view.width, view.height])?;
        for ident in [&view.name, &view.schema] {
            if !valid_ident(ident) {
                return Err(format!("Nome de view inválido: {ident}"));
            }
        }
        if !names.insert((view.schema.as_str(), view.name.as_str()))
            || drafts.contains(&(view.schema.as_str(), view.name.as_str()))
        {
            return Err(format!("A view {}.{} se repete", view.schema, view.name));
        }
    }
    for view in &def.views {
        let missing = view
            .depends_on_tables
            .iter()
            .find(|a| !aliases.contains(a.as_str()))
            .or_else(|| {
                view.depends_on_views
                    .iter()
                    .find(|v| **v == view.id || !view_ids.contains(v.as_str()))
            });
        if let Some(dep) = missing {
            return Err(format!(
                "A view {} depende de {dep}, que não está no model",
                view.name
            ));
        }
    }

    Ok(())
}

/// A join oriented from the primary: `from` is already in the tree.
struct Step<'a> {
    from: &'a End,
    to: &'a End,
    relationship: &'a str,
    join: &'a str,
}

/// The cube YAML for `def`, checked against the connection's `columns`
/// (keyed by plain schema name), preceded by the header line.
pub fn build(
    name: &str,
    model_id: Uuid,
    cube_name: &str,
    def: &Definition,
    columns: &cubedriver::DatabaseStructure,
    db_type: &str,
) -> Result<String, String> {
    if let Some(t) = def.tables.iter().find(|t| t.draft.is_some()) {
        return Err(format!(
            "A tabela {}.{} ainda não existe no banco: use Criar no banco",
            t.schema, t.table
        ));
    }
    validate(def)?;
    let primary = def.primary.as_deref().ok_or("Escolha a tabela principal")?;
    let primary_key = def
        .primary_key
        .as_deref()
        .ok_or("Escolha a chave primária")?;

    let mut tables = HashMap::new();
    for table in &def.tables {
        let cols = columns
            .get(&table.schema)
            .and_then(|t| t.get(&table.table))
            .ok_or_else(|| {
                format!(
                    "A tabela {}.{} não existe na conexão {}",
                    table.schema, table.table, def.data_source
                )
            })?;
        tables.insert(table.alias.as_str(), (table, cols));
    }
    let column_of = |end: &End| {
        tables[end.alias.as_str()]
            .1
            .iter()
            .find(|c| c.name == end.column)
            .ok_or_else(|| format!("A coluna {}.{} não existe", end.alias, end.column))
    };
    for join in &def.joins {
        column_of(&join.from)?;
        column_of(&join.to)?;
    }
    let key = column_of(&End {
        alias: primary.into(),
        column: primary_key.into(),
    })?;
    if cube_type(&key.type_).is_none() {
        return Err(format!(
            "A chave primária {primary_key} não tem tipo no Cube"
        ));
    }

    // Breadth-first from the primary; validate made the links a forest, and
    // a table this walk does not reach is not linked to the primary.
    let mut order = vec![primary];
    let mut visited = HashSet::from([primary]);
    let mut steps = Vec::new();
    let mut queue = VecDeque::from([primary]);
    while let Some(alias) = queue.pop_front() {
        for join in &def.joins {
            let step = if join.from.alias == alias && !visited.contains(join.to.alias.as_str()) {
                Step {
                    from: &join.from,
                    to: &join.to,
                    relationship: &join.relationship,
                    join: &join.join,
                }
            } else if join.to.alias == alias && !visited.contains(join.from.alias.as_str()) {
                Step {
                    from: &join.to,
                    to: &join.from,
                    relationship: match join.relationship.as_str() {
                        "one_to_many" => "many_to_one",
                        "many_to_one" => "one_to_many",
                        other => other,
                    },
                    join: &join.join,
                }
            } else {
                continue;
            };
            visited.insert(step.to.alias.as_str());
            queue.push_back(step.to.alias.as_str());
            order.push(step.to.alias.as_str());
            steps.push(step);
        }
    }
    if let Some(table) = def
        .tables
        .iter()
        .find(|t| !visited.contains(t.alias.as_str()))
    {
        return Err(format!(
            "A tabela {} não está ligada à principal",
            table.alias
        ));
    }
    let multiplies = steps.iter().any(|s| s.relationship == "one_to_many");

    let q = |ident: &str| quote(db_type, ident);
    let mut members = BTreeSet::from(["count".to_string()]);
    let mut select = Vec::new();
    let mut dimensions = Vec::new();
    let mut key_sql = String::new();
    for alias in &order {
        let (_, cols) = tables[alias];
        for column in cols {
            let Some(cube_type) = cube_type(&column.type_) else {
                continue;
            };
            let base = format!("{}_{}", snake(alias), snake(&column.name));
            let mut member = base.clone();
            let mut n = 2;
            while !members.insert(member.clone()) {
                member = format!("{base}_{n}");
                n += 1;
            }
            select.push(format!(
                "{}.{} AS {}",
                q(alias),
                q(&column.name),
                q(&member)
            ));

            let sql = format!("{{CUBE}}.{}", q(&member));
            let mut dimension = Mapping::new();
            dimension.insert("name".into(), member.into());
            dimension.insert("sql".into(), sql.clone().into());
            dimension.insert("type".into(), cube_type.into());
            if *alias == primary && column.name == primary_key {
                dimension.insert("primary_key".into(), true.into());
                key_sql = sql;
            }
            dimensions.push(Yaml::Mapping(dimension));
        }
    }

    let from = |alias: &str| {
        let (table, _) = tables[alias];
        format!("{}.{} AS {}", q(&table.schema), q(&table.table), q(alias))
    };
    let mut sql = vec![
        format!("SELECT {}", select.join(", ")),
        format!("FROM {}", from(primary)),
    ];
    for step in &steps {
        sql.push(format!(
            "{} JOIN {} ON {}.{} = {}.{}",
            step.join.to_ascii_uppercase(),
            from(&step.to.alias),
            q(&step.from.alias),
            q(&step.from.column),
            q(&step.to.alias),
            q(&step.to.column),
        ));
    }

    let mut cube = Mapping::new();
    cube.insert("name".into(), cube_name.into());
    if def.data_source != "default" {
        cube.insert("data_source".into(), def.data_source.as_str().into());
    }
    cube.insert("sql".into(), sql.join("\n").into());
    cube.insert("dimensions".into(), Yaml::Sequence(dimensions));
    let mut count = Mapping::new();
    count.insert("name".into(), "count".into());
    if multiplies {
        // A one_to_many repeats the primary's rows: count its key, once.
        count.insert("type".into(), "count_distinct".into());
        count.insert("sql".into(), key_sql.into());
    } else {
        count.insert("type".into(), "count".into());
    }
    cube.insert(
        "measures".into(),
        Yaml::Sequence(vec![Yaml::Mapping(count)]),
    );

    let mut doc = Mapping::new();
    doc.insert("cubes".into(), Yaml::Sequence(vec![Yaml::Mapping(cube)]));
    let yaml = serde_yaml::to_string(&doc).map_err(|e| e.to_string())?;
    Ok(format!(
        "# Built from model {name} ({model_id}): Build cube rewrites this file\n{yaml}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cubedriver::SchemaColumn;

    fn col(name: &str, type_: &str) -> SchemaColumn {
        SchemaColumn {
            name: name.into(),
            type_: type_.into(),
            attributes: vec![],
            foreign_keys: vec![],
        }
    }

    fn columns() -> cubedriver::DatabaseStructure {
        let mut tables = std::collections::BTreeMap::new();
        tables.insert(
            "orders".to_string(),
            vec![
                col("id", "int"),
                col("customer_id", "int"),
                col("Order ID", "text"),
                col("order_id", "int"),
            ],
        );
        tables.insert(
            "customers".to_string(),
            vec![col("id", "int"), col("region", "text")],
        );
        let mut db = cubedriver::DatabaseStructure::new();
        db.insert("public".to_string(), tables);
        db
    }

    fn table(alias: &str, table: &str) -> ModelTable {
        ModelTable {
            alias: alias.into(),
            schema: "public".into(),
            table: table.into(),
            x: 0.0,
            y: 0.0,
            color: None,
            draft: None,
            ..Default::default()
        }
    }

    fn join(from: (&str, &str), to: (&str, &str), relationship: &str, kind: &str) -> ModelJoin {
        ModelJoin {
            from: End {
                alias: from.0.into(),
                column: from.1.into(),
            },
            to: End {
                alias: to.0.into(),
                column: to.1.into(),
            },
            relationship: relationship.into(),
            join: kind.into(),
            ..Default::default()
        }
    }

    /// orders (primary, pk id) many_to_one customers, left.
    fn orders_customers() -> Definition {
        Definition {
            data_source: "default".into(),
            tables: vec![table("orders", "orders"), table("customers", "customers")],
            primary: Some("orders".into()),
            primary_key: Some("id".into()),
            joins: vec![join(
                ("orders", "customer_id"),
                ("customers", "id"),
                "many_to_one",
                "left",
            )],
            ..Default::default()
        }
    }

    fn run(def: &Definition) -> Result<String, String> {
        build("Sales", Uuid::nil(), "sales", def, &columns(), "postgres")
    }

    fn cube(yaml: &str) -> Yaml {
        let doc: Yaml = serde_yaml::from_str(yaml).unwrap();
        doc["cubes"][0].clone()
    }

    fn member<'a>(cube: &'a Yaml, kind: &str, name: &str) -> &'a Yaml {
        cube[kind]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|m| m["name"].as_str() == Some(name))
            .unwrap_or_else(|| panic!("no {kind} {name} in {cube:?}"))
    }

    fn sql(cube: &Yaml) -> String {
        cube["sql"].as_str().unwrap().to_string()
    }

    #[test]
    fn builds_a_left_join_cube() {
        let yaml = run(&orders_customers()).unwrap();
        assert!(yaml.starts_with(&format!(
            "# Built from model Sales ({}): Build cube rewrites this file\n",
            Uuid::nil()
        )));
        let cube = cube(&yaml);
        assert_eq!(cube["name"].as_str(), Some("sales"));
        let sql = sql(&cube);
        assert!(
            sql.contains(r#"FROM "public"."orders" AS "orders""#),
            "{sql}"
        );
        assert!(
            sql.contains(
                r#"LEFT JOIN "public"."customers" AS "customers" ON "orders"."customer_id" = "customers"."id""#
            ),
            "{sql}"
        );
        assert!(
            sql.contains(r#""customers"."region" AS "customers_region""#),
            "{sql}"
        );
        let region = member(&cube, "dimensions", "customers_region");
        assert_eq!(region["type"].as_str(), Some("string"));
        assert_eq!(region["sql"].as_str(), Some(r#"{CUBE}."customers_region""#));
        let id = member(&cube, "dimensions", "orders_id");
        assert_eq!(id["primary_key"].as_bool(), Some(true));
        assert_eq!(id["type"].as_str(), Some("number"));
        assert_eq!(
            member(&cube, "dimensions", "customers_id")["primary_key"].as_bool(),
            None
        );
        let count = member(&cube, "measures", "count");
        assert_eq!(count["type"].as_str(), Some("count"));
        assert!(count["sql"].is_null());
    }

    #[test]
    fn a_one_to_many_join_counts_distinct() {
        let def = Definition {
            primary: Some("customers".into()),
            joins: vec![join(
                ("customers", "id"),
                ("orders", "customer_id"),
                "one_to_many",
                "left",
            )],
            ..orders_customers()
        };
        let cube = cube(&run(&def).unwrap());
        assert!(sql(&cube).contains(r#"FROM "public"."customers" AS "customers""#));
        let count = member(&cube, "measures", "count");
        assert_eq!(count["type"].as_str(), Some("count_distinct"));
        assert_eq!(count["sql"].as_str(), Some(r#"{CUBE}."customers_id""#));
    }

    #[test]
    fn a_backwards_join_is_flipped() {
        let def = Definition {
            joins: vec![join(
                ("customers", "id"),
                ("orders", "customer_id"),
                "one_to_many",
                "left",
            )],
            ..orders_customers()
        };
        let cube = cube(&run(&def).unwrap());
        let sql = sql(&cube);
        assert!(
            sql.contains(r#"FROM "public"."orders" AS "orders""#),
            "{sql}"
        );
        assert!(
            sql.contains(
                r#"LEFT JOIN "public"."customers" AS "customers" ON "orders"."customer_id" = "customers"."id""#
            ),
            "{sql}"
        );
        assert_eq!(
            member(&cube, "measures", "count")["type"].as_str(),
            Some("count")
        );
    }

    #[test]
    fn an_inner_join_is_inner() {
        let mut def = orders_customers();
        def.joins[0].join = "inner".into();
        let sql = sql(&cube(&run(&def).unwrap()));
        assert!(
            sql.contains(r#"INNER JOIN "public"."customers" AS "customers""#),
            "{sql}"
        );
    }

    #[test]
    fn colliding_names_get_a_suffix() {
        let cube = cube(&run(&orders_customers()).unwrap());
        let sql = sql(&cube);
        assert!(
            sql.contains(r#""orders"."Order ID" AS "orders_order_id""#),
            "{sql}"
        );
        assert!(
            sql.contains(r#""orders"."order_id" AS "orders_order_id_2""#),
            "{sql}"
        );
        assert_eq!(
            member(&cube, "dimensions", "orders_order_id")["type"].as_str(),
            Some("string")
        );
        assert_eq!(
            member(&cube, "dimensions", "orders_order_id_2")["type"].as_str(),
            Some("number")
        );
    }

    #[test]
    fn the_same_table_twice_uses_both_aliases() {
        let def = Definition {
            tables: vec![
                table("orders", "orders"),
                table("customers", "customers"),
                table("customers_2", "customers"),
            ],
            joins: vec![
                join(
                    ("orders", "customer_id"),
                    ("customers", "id"),
                    "many_to_one",
                    "left",
                ),
                join(
                    ("orders", "order_id"),
                    ("customers_2", "id"),
                    "many_to_one",
                    "left",
                ),
            ],
            ..orders_customers()
        };
        let cube = cube(&run(&def).unwrap());
        let sql = sql(&cube);
        assert!(
            sql.contains(
                r#"LEFT JOIN "public"."customers" AS "customers_2" ON "orders"."order_id" = "customers_2"."id""#
            ),
            "{sql}"
        );
        member(&cube, "dimensions", "customers_region");
        member(&cube, "dimensions", "customers_2_region");
    }

    #[test]
    fn validate_refuses_bad_shapes() {
        let base = orders_customers();
        assert_eq!(validate(&base), Ok(()));

        let mut unlinked = base.clone();
        unlinked.tables.push(table("x", "customers"));
        // A draft: the table just dropped is not linked yet.
        assert_eq!(validate(&unlinked), Ok(()));

        let mut twice = base.clone();
        twice.joins.push(join(
            ("customers", "id"),
            ("orders", "id"),
            "one_to_one",
            "left",
        ));
        assert_eq!(
            validate(&twice),
            Err("customers e orders já estão ligadas".into())
        );

        let mut cycle = base.clone();
        cycle.tables.push(table("c2", "customers"));
        cycle.joins.push(join(
            ("orders", "order_id"),
            ("c2", "id"),
            "many_to_one",
            "left",
        ));
        cycle.joins.push(join(
            ("c2", "id"),
            ("customers", "id"),
            "one_to_one",
            "left",
        ));
        assert_eq!(
            validate(&cycle),
            Err("c2 e customers já estão ligadas".into())
        );

        let mut relationship = base.clone();
        relationship.joins[0].relationship = "many_to_many".into();
        assert!(validate(&relationship).is_err());

        let mut kind = base.clone();
        kind.joins[0].join = "full".into();
        assert!(validate(&kind).is_err());

        let mut duplicate = base.clone();
        duplicate.tables.push(table("orders", "customers"));
        assert!(validate(&duplicate).is_err());

        let mut empty_alias = base.clone();
        empty_alias.tables[1].alias = "".into();
        assert!(validate(&empty_alias).is_err());

        let mut unknown_primary = base.clone();
        unknown_primary.primary = Some("nope".into());
        assert!(validate(&unknown_primary).is_err());

        let mut unknown_alias = base.clone();
        unknown_alias.joins[0].to.alias = "nope".into();
        assert!(validate(&unknown_alias).is_err());
    }

    #[test]
    fn an_empty_model_is_a_valid_draft() {
        let def = Definition {
            data_source: "default".into(),
            tables: vec![],
            primary: None,
            primary_key: None,
            joins: vec![],
            ..Default::default()
        };
        assert_eq!(validate(&def), Ok(()));
    }

    #[test]
    fn build_requires_primary_and_key_and_existing_columns() {
        let mut no_primary = orders_customers();
        no_primary.primary = None;
        assert_eq!(run(&no_primary), Err("Escolha a tabela principal".into()));

        let mut no_key = orders_customers();
        no_key.primary_key = None;
        assert_eq!(run(&no_key), Err("Escolha a chave primária".into()));

        let mut no_column = orders_customers();
        no_column.joins[0].to.column = "nope".into();
        assert_eq!(
            run(&no_column),
            Err("A coluna customers.nope não existe".into())
        );

        let mut no_key_column = orders_customers();
        no_key_column.primary_key = Some("nope".into());
        assert_eq!(
            run(&no_key_column),
            Err("A coluna orders.nope não existe".into())
        );

        let mut unlinked = orders_customers();
        unlinked.tables.push(table("x", "customers"));
        assert_eq!(
            run(&unlinked),
            Err("A tabela x não está ligada à principal".into())
        );

        let mut no_table = orders_customers();
        no_table.tables[1].table = "gone".into();
        assert_eq!(
            run(&no_table),
            Err("A tabela public.gone não existe na conexão default".into())
        );
    }

    #[test]
    fn an_untyped_primary_key_is_refused() {
        let mut db = columns();
        db.get_mut("public")
            .unwrap()
            .get_mut("orders")
            .unwrap()
            .push(col("tags", "text[]"));
        let mut def = orders_customers();
        def.primary_key = Some("tags".into());
        assert_eq!(
            build("Sales", Uuid::nil(), "sales", &def, &db, "postgres"),
            Err("A chave primária tags não tem tipo no Cube".into())
        );
    }

    #[test]
    fn the_data_source_line() {
        let yaml = run(&orders_customers()).unwrap();
        assert!(!yaml.contains("data_source"), "{yaml}");

        let def = Definition {
            data_source: "pg".into(),
            ..orders_customers()
        };
        let yaml = run(&def).unwrap();
        assert!(yaml.contains("data_source: pg"), "{yaml}");
    }

    fn dcol(name: &str, kind: &str, nullable: bool, pk: bool) -> DraftColumn {
        DraftColumn {
            name: name.into(),
            kind: kind.into(),
            nullable,
            primary_key: pk,
            ..Default::default()
        }
    }

    fn draft_table(alias: &str, name: &str, columns: Vec<DraftColumn>) -> ModelTable {
        ModelTable {
            draft: Some(DraftTable {
                columns,
                ..Default::default()
            }),
            ..table(alias, name)
        }
    }

    fn items_def() -> Definition {
        Definition {
            data_source: "ds".into(),
            tables: vec![draft_table(
                "i",
                "items",
                vec![
                    dcol("id", "integer", false, true),
                    dcol("order", "text", true, false),
                    dcol("price", "numeric", false, false),
                ],
            )],
            primary: None,
            primary_key: None,
            joins: vec![],
            ..Default::default()
        }
    }

    fn with_draft(t: ModelTable) -> Definition {
        Definition {
            tables: vec![t],
            ..items_def()
        }
    }

    #[test]
    fn old_definitions_still_parse() {
        let json = r#"{"dataSource":"ds","tables":[{"alias":"o","schema":"public","table":"orders","x":1.0,"y":2.0}],"primary":null,"primaryKey":null,"joins":[]}"#;
        let def: Definition = serde_json::from_str(json).unwrap();
        assert_eq!(def.tables[0].color, None);
        assert_eq!(def.tables[0].draft, None);
        let out = serde_json::to_string(&def).unwrap();
        assert!(!out.contains("color") && !out.contains("draft"), "{out}");
    }

    #[test]
    fn validate_checks_drafts() {
        let ok = vec![dcol("id", "integer", false, true)];
        let bad = |t: ModelTable| validate(&with_draft(t));
        let e = bad(draft_table("i", "1x", ok.clone())).unwrap_err();
        assert!(e.contains("inválido"), "{e}");
        assert!(bad(draft_table(
            "i",
            "items",
            vec![dcol("id", "nope", false, true)]
        ))
        .is_err());
        assert!(bad(draft_table("i", "items", vec![])).is_err());
        assert!(bad(draft_table(
            "i",
            "items",
            vec![
                dcol("id", "integer", false, true),
                dcol("id", "text", true, false)
            ]
        ))
        .is_err());
        let mut twice = with_draft(draft_table("i", "items", ok.clone()));
        twice.tables.push(draft_table("j", "items", ok.clone()));
        assert!(validate(&twice).is_err());
        let mut orange = draft_table("i", "items", ok.clone());
        orange.color = Some("orange".into());
        assert!(bad(orange).is_err());
        let mut blue = draft_table("i", "items", ok);
        blue.color = Some("blue".into());
        assert!(bad(blue).is_ok());
    }

    #[test]
    fn build_refuses_drafts() {
        let mut def = items_def();
        def.tables.insert(0, table("o", "orders"));
        def.primary = Some("o".into());
        def.primary_key = Some("id".into());
        let err = build("m", Uuid::nil(), "m", &def, &columns(), "postgres").unwrap_err();
        assert_eq!(
            err,
            "A tabela public.items ainda não existe no banco: use Criar no banco"
        );
    }

    #[test]
    fn postgres_create_statement() {
        let got = create_statements(&items_def(), "postgres").unwrap();
        assert_eq!(got[0].table.as_deref(), Some("public.items"));
        assert_eq!(
            got[0].sql,
            r#"CREATE TABLE "public"."items" ("id" integer NOT NULL, "order" text, "price" numeric(18,4) NOT NULL, PRIMARY KEY ("id"))"#
        );
    }

    #[test]
    fn mysql_create_statement() {
        let mut def = items_def();
        def.tables[0]
            .draft
            .as_mut()
            .unwrap()
            .columns
            .push(dcol("at", "timestamp", true, false));
        let got = create_statements(&def, "mysql").unwrap();
        assert_eq!(
            got[0].sql,
            "CREATE TABLE `public`.`items` (`id` int NOT NULL, `order` text, `price` decimal(18,4) NOT NULL, `at` datetime, PRIMARY KEY (`id`))"
        );
    }

    #[test]
    fn mysql_refuses_a_text_primary_key() {
        let def = with_draft(draft_table(
            "i",
            "items",
            vec![dcol("code", "text", false, true)],
        ));
        assert_eq!(
            create_statements(&def, "mysql").unwrap_err(),
            "No MySQL a chave primária public.items.code não pode ser text: use integer, bigint ou outro tipo"
        );
        assert!(create_statements(&def, "postgres").is_ok());
    }

    #[test]
    fn duplicate_column_names_ignore_case() {
        let def = with_draft(draft_table(
            "i",
            "items",
            vec![
                dcol("Id", "integer", false, true),
                dcol("id", "text", true, false),
            ],
        ));
        assert_eq!(
            create_statements(&def, "postgres").unwrap_err(),
            "A coluna id se repete em public.items"
        );
    }

    #[test]
    fn unsupported_db_type() {
        assert_eq!(
            create_statements(&items_def(), "bigquery").unwrap_err(),
            "Criar tabelas não é suportado para bigquery"
        );
    }

    #[test]
    fn primary_key_is_not_null_and_optional() {
        let def = with_draft(draft_table(
            "i",
            "items",
            vec![
                dcol("id", "integer", true, true),
                dcol("n", "text", true, false),
            ],
        ));
        let sql = &create_statements(&def, "postgres").unwrap()[0].sql;
        assert!(sql.contains(r#""id" integer NOT NULL"#), "{sql}");
        let def = with_draft(draft_table(
            "i",
            "items",
            vec![dcol("n", "text", true, false)],
        ));
        let sql = &create_statements(&def, "postgres").unwrap()[0].sql;
        assert!(!sql.contains("PRIMARY KEY"), "{sql}");
    }

    fn v2_json() -> serde_json::Value {
        serde_json::json!({
            "formatVersion": 2,
            "dataSource": "default",
            "description": "Sales model",
            "tables": [{
                "alias": "orders", "schema": "public", "table": "orders", "x": 0.0, "y": 0.0,
                "comment": "every order", "width": 220.0, "height": 140.0,
                "shortcuts": [{ "id": "s1", "x": 400.0, "y": 10.0 }]
            }, {
                "alias": "customers", "schema": "public", "table": "customers", "x": 300.0, "y": 0.0
            }],
            "primary": "orders",
            "primaryKey": "id",
            "joins": [{
                "from": { "alias": "orders", "column": "customer_id" },
                "to": { "alias": "customers", "column": "id" },
                "relationship": "many_to_one", "join": "left",
                "points": [{ "x": 250.0, "y": 40.0 }], "swapped": true, "hidden": true
            }]
        })
    }

    #[test]
    fn accepts_v2_fields() {
        let value = v2_json();
        let def: Definition = serde_json::from_value(value.clone()).unwrap();
        validate(&def).unwrap();
        assert_eq!(serde_json::to_value(&def).unwrap(), value);
    }

    #[test]
    fn refuses_bad_v2_fields() {
        let bad = |edit: &dyn Fn(&mut serde_json::Value)| {
            let mut value = v2_json();
            edit(&mut value);
            let def: Definition = serde_json::from_value(value).unwrap();
            validate(&def).unwrap_err()
        };
        let e = bad(&|v| v["formatVersion"] = 3.into());
        assert!(e.contains("formatVersion inválido: 3"), "{e}");
        let e = bad(&|v| v["tables"][0]["width"] = 0.0.into());
        assert!(e.contains("Tamanho inválido"), "{e}");
        let e = bad(&|v| v["tables"][0]["height"] = (-5.0).into());
        assert!(e.contains("Tamanho inválido"), "{e}");
        let e = bad(&|v| {
            v["tables"][1]["shortcuts"] = serde_json::json!([{ "id": "s1", "x": 1.0, "y": 1.0 }])
        });
        assert!(e.contains("O atalho s1 se repete"), "{e}");
        let mut def: Definition = serde_json::from_value(v2_json()).unwrap();
        def.joins[0].points[0].x = f64::INFINITY;
        let e = validate(&def).unwrap_err();
        assert!(e.contains("Ponto inválido"), "{e}");
    }

    #[test]
    fn v1_definition_still_valid() {
        let value = serde_json::to_value(orders_customers()).unwrap();
        let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["dataSource", "joins", "primary", "primaryKey", "tables"]
        );
        assert_eq!(value["tables"][0].as_object().unwrap().len(), 5);
        assert_eq!(value["joins"][0].as_object().unwrap().len(), 4);
    }

    fn physical_json() -> serde_json::Value {
        serde_json::json!({
            "formatVersion": 2,
            "dataSource": "default",
            "tables": [{
                "alias": "items", "schema": "shop", "table": "items", "x": 0.0, "y": 0.0,
                "draft": {
                    "columns": [
                        { "name": "id", "type": "integer", "nullable": false, "primaryKey": true, "autoIncrement": true },
                        { "name": "sku", "type": "varchar(32)", "nullable": false, "primaryKey": false, "comment": "stock unit", "default": "'x'", "check": "sku <> ''" },
                        { "name": "price", "type": "numeric(10,2)", "nullable": true, "primaryKey": false, "unsigned": true, "zerofill": true }
                    ],
                    "primaryKeyName": "pk_items",
                    "uniqueKeys": [{ "name": "uk_sku", "columns": ["sku"] }],
                    "indexes": [{ "name": "ix_price", "columns": [{ "name": "price", "desc": true }], "unique": false }],
                    "checks": [{ "name": "ck_price", "expression": "price >= 0" }],
                    "scriptBefore": "-- before",
                    "scriptAfter": "-- after"
                }
            }, {
                "alias": "orders", "schema": "shop", "table": "orders", "x": 300.0, "y": 0.0
            }],
            "primary": "orders",
            "primaryKey": "id",
            "joins": [{
                "from": { "alias": "orders", "column": "item_id" },
                "to": { "alias": "items", "column": "id" },
                "relationship": "many_to_one", "join": "inner",
                "id": "r1", "name": "fk_orders_items", "comment": "c",
                "cardinality": "0..*", "mandatory": true,
                "onUpdate": "cascade", "onDelete": "set null",
                "fromRole": "buyer", "toRole": "item",
                "pairs": [{ "from": "item_sku", "to": "sku" }]
            }],
            "settings": {
                "validation": { "table.hasPk": false },
                "format": { "showTypes": true, "showNullable": false },
                "sqlGeneration": { "drop": true, "comments": true },
                "scripts": { "createBefore": "a", "createAfter": "b", "dropBefore": "c", "dropAfter": "d" }
            }
        })
    }

    #[test]
    fn accepts_physical_fields() {
        let value = physical_json();
        let def: Definition = serde_json::from_value(value.clone()).unwrap();
        validate(&def).unwrap();
        assert_eq!(serde_json::to_value(&def).unwrap(), value);
    }

    #[test]
    fn refuses_bad_physical_fields() {
        let bad = |edit: &dyn Fn(&mut serde_json::Value)| {
            let mut value = physical_json();
            edit(&mut value);
            let def: Definition = serde_json::from_value(value).unwrap();
            validate(&def).unwrap_err()
        };
        let e = bad(&|v| v["tables"][0]["draft"]["uniqueKeys"][0]["columns"] = serde_json::json!([]));
        assert!(e.contains("A chave única uk_sku precisa de colunas"), "{e}");
        let e = bad(&|v| v["tables"][0]["draft"]["indexes"][0]["columns"][0]["name"] = "nope".into());
        assert!(e.contains("O índice ix_price usa a coluna nope, que não existe"), "{e}");
        let e = bad(&|v| v["joins"][0]["cardinality"] = "2..*".into());
        assert!(e.contains("Cardinalidade inválida"), "{e}");
        let e = bad(&|v| v["joins"][0]["onDelete"] = "drop".into());
        assert!(e.contains("Ação inválida"), "{e}");
        let e = bad(&|v| {
            let j = v["joins"][0].clone();
            let mut second = j.clone();
            second["from"]["alias"] = "items".into();
            second["to"]["alias"] = "items".into();
            v["joins"] = serde_json::json!([j, second]);
        });
        assert!(e.contains("A ligação r1 se repete"), "{e}");
        let e = bad(&|v| v["tables"][0]["draft"]["checks"][0]["name"] = "1ck".into());
        assert!(e.contains("Nome inválido"), "{e}");
        let e = bad(&|v| v["tables"][0]["draft"]["uniqueKeys"] = serde_json::json!([
            { "name": "uk", "columns": ["sku"] }, { "name": "uk", "columns": ["price"] }
        ]));
        assert!(e.contains("O nome uk se repete em shop.items"), "{e}");
    }

    #[test]
    fn dialect_types() {
        assert_eq!(dialect("postgres"), Some("postgres"));
        assert_eq!(dialect("mysql"), Some("mysql"));
        assert_eq!(dialect("clickhouse"), None);
        for ok in ["varchar(64)", "numeric(18,4)", "numeric", "double precision", "timestamptz", "integer"] {
            assert!(valid_type(Some("postgres"), ok), "{ok}");
        }
        for bad in ["varchar(x)", "int", "varchar(0)", "numeric(1,2,3)", "jsonb(2)"] {
            assert!(!valid_type(Some("postgres"), bad), "{bad}");
        }
        for ok in ["int", "varchar(255)", "decimal(10, 2)", "datetime", "integer", "numeric"] {
            assert!(valid_type(Some("mysql"), ok), "{ok}");
        }
        assert!(!valid_type(Some("mysql"), "varchar"));
        assert!(!valid_type(Some("mysql"), "jsonb"));
        assert!(valid_type(None, "integer"));
        assert!(!valid_type(None, "jsonb"));
    }

    #[test]
    fn validate_types_names_the_column() {
        let mut value = physical_json();
        value["tables"][0]["draft"]["columns"][1]["type"] = "jsonb".into();
        let def: Definition = serde_json::from_value(value).unwrap();
        let e = validate_types(&def, "mysql").unwrap_err();
        assert_eq!(e, "Tipo inválido para shop.items.sku: jsonb");
        validate_types(&serde_json::from_value(physical_json()).unwrap(), "postgres").unwrap();
    }

    fn objects_json() -> serde_json::Value {
        let mut v = v2_json();
        v["notes"] = serde_json::json!([{
            "id": "n1", "x": 1.0, "y": 2.0, "width": 200.0, "height": 100.0, "z": 3,
            "text": "hello", "color": "amber"
        }]);
        v["areas"] = serde_json::json!([{
            "id": "a1", "name": "Sales", "comment": "c", "x": 0.0, "y": 0.0,
            "width": 400.0, "height": 300.0, "z": 1, "fill": "blue", "border": "red"
        }]);
        v["views"] = serde_json::json!([{
            "id": "v1", "name": "recent", "schema": "public", "comment": "c",
            "sql": "select 1", "columns": [{ "name": "id", "type": "integer" }],
            "dependsOnTables": ["orders"], "dependsOnViews": [],
            "scriptBefore": "-- a", "scriptAfter": "-- b",
            "x": 5.0, "y": 6.0, "width": 180.0, "height": 90.0, "z": 2
        }, {
            "id": "v2", "name": "older", "schema": "public", "sql": "select 2",
            "columns": [], "dependsOnTables": [], "dependsOnViews": ["v1"],
            "x": 5.0, "y": 6.0
        }]);
        v
    }

    #[test]
    fn accepts_notes_areas_views() {
        let value = objects_json();
        let def: Definition = serde_json::from_value(value.clone()).unwrap();
        validate(&def).unwrap();
        assert_eq!(serde_json::to_value(&def).unwrap(), value);
        let old = serde_json::to_value(orders_customers()).unwrap();
        let keys = old.as_object().unwrap();
        for k in ["notes", "areas", "views"] {
            assert!(!keys.contains_key(k), "{k}");
        }
    }

    #[test]
    fn refuses_bad_objects() {
        let bad = |edit: &dyn Fn(&mut serde_json::Value)| {
            let mut value = objects_json();
            edit(&mut value);
            let def: Definition = serde_json::from_value(value).unwrap();
            validate(&def).unwrap_err()
        };
        let e = bad(&|v| {
            let first = v["notes"][0].clone();
            v["notes"].as_array_mut().unwrap().push(first);
        });
        assert_eq!(e, "O objeto n1 se repete");
        let e = bad(&|v| {
            let first = v["areas"][0].clone();
            v["areas"].as_array_mut().unwrap().push(first);
        });
        assert_eq!(e, "O objeto a1 se repete");
        let e = bad(&|v| v["views"][1]["id"] = "v1".into());
        assert_eq!(e, "O objeto v1 se repete");
        let e = bad(&|v| v["areas"][0]["width"] = 0.0.into());
        assert_eq!(e, "Tamanho inválido em a1: 0");
        let e = bad(&|v| v["notes"][0]["height"] = (-1.0).into());
        assert_eq!(e, "Tamanho inválido em n1: -1");
        let e = bad(&|v| v["areas"][0]["fill"] = "pink2".into());
        assert_eq!(e, "Cor inválida: pink2");
        let e = bad(&|v| v["areas"][0]["border"] = "#fff".into());
        assert_eq!(e, "Cor inválida: #fff");
        let e = bad(&|v| v["notes"][0]["color"] = "nope".into());
        assert_eq!(e, "Cor inválida: nope");
        let e = bad(&|v| v["views"][0]["name"] = "1x".into());
        assert_eq!(e, "Nome de view inválido: 1x");
        let e = bad(&|v| v["views"][0]["schema"] = "a-b".into());
        assert_eq!(e, "Nome de view inválido: a-b");
        let e = bad(&|v| v["views"][1]["name"] = "recent".into());
        assert_eq!(e, "A view public.recent se repete");
        let e = bad(&|v| {
            v["tables"][1]["draft"] = serde_json::json!({
                "columns": [{ "name": "id", "type": "integer", "nullable": false }]
            });
            v["tables"][1]["schema"] = "public".into();
            v["tables"][1]["table"] = "recent".into();
        });
        assert_eq!(e, "A view public.recent se repete");
        let e = bad(&|v| v["views"][0]["dependsOnTables"] = serde_json::json!(["ghost"]));
        assert_eq!(e, "A view recent depende de ghost, que não está no model");
        let e = bad(&|v| v["views"][1]["dependsOnViews"] = serde_json::json!(["v9"]));
        assert_eq!(e, "A view older depende de v9, que não está no model");
        let e = bad(&|v| v["views"][1]["dependsOnViews"] = serde_json::json!(["v2"]));
        assert_eq!(e, "A view older depende de v2, que não está no model");
    }

    #[test]
    fn validate_types_checks_view_columns() {
        let mut value = objects_json();
        value["views"][0]["columns"][0]["type"] = "jsonb".into();
        let def: Definition = serde_json::from_value(value.clone()).unwrap();
        validate_types(&def, "postgres").unwrap();
        value["views"][0]["columns"][0]["type"] = "int".into();
        let def: Definition = serde_json::from_value(value).unwrap();
        assert_eq!(
            validate_types(&def, "postgres").unwrap_err(),
            "Tipo inválido para recent.id: int"
        );
    }
}
