//! Models: tables on a canvas, linked by their columns, and cubes over them,
//! each built into its own YAML. Pure: [`validate`] checks the shape, [`build_all`]
//! checks it against the connection's columns and writes one YAML per cube.

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cubes: Vec<CubeNode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cube_joins: Vec<CubeJoin>,
}

pub const DIMENSION_TYPES: [&str; 5] = ["string", "number", "time", "boolean", "geo"];
pub const MEASURE_TYPES: [&str; 8] = [
    "count",
    "count_distinct",
    "count_distinct_approx",
    "sum",
    "avg",
    "min",
    "max",
    "number",
];
pub const MEASURE_FORMATS: [&str; 2] = ["percent", "currency"];
pub const SOURCE_KINDS: [&str; 3] = ["table", "joined", "sql"];

/// A cube drawn on the canvas, with its own members.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CubeNode {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A palette name from [`COLORS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    pub source: CubeSource,
    pub dimensions: Vec<CubeDimension>,
    pub measures: Vec<CubeMeasure>,
    pub segments: Vec<CubeSegment>,
    pub x: f64,
    pub y: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z: Option<i32>,
}

/// Where a cube reads from. `kind` is one of [`SOURCE_KINDS`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CubeSource {
    pub kind: String,
    /// The table alias for `table`, the primary table for `joined`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub table: Option<String>,
    /// A column of `table`, used in auto mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_key: Option<String>,
    /// The query of a `sql` cube.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,
    /// Aliases a `sql` cube reads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tables: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CubeDimension {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    /// `alias.column` in a `joined` cube, a plain column otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub primary_key: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CubeMeasure {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CubeSegment {
    pub name: String,
    pub sql: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// A reference between two cubes; `on` pairs dimensions of `from` and `to`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CubeJoin {
    pub id: String,
    /// Cube ids.
    pub from: String,
    pub to: String,
    pub relationship: String,
    pub on: Vec<ColumnPair>,
    pub points: Vec<Point>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

/// Turns the old single-cube shape into a cube on the canvas: a set
/// `primary` becomes cube `c1` (when there are no cubes yet), and `primary`
/// and `primaryKey` are cleared either way.
pub fn upgrade(def: &mut Definition, cube_name: &str) {
    if let Some(primary) = def.primary.take() {
        if def.cubes.is_empty() {
            let x = def.tables.iter().map(|t| t.x).fold(0.0, f64::max) + 320.0;
            let y = def
                .tables
                .iter()
                .find(|t| t.alias == primary)
                .map_or(0.0, |t| t.y);
            def.cubes.push(CubeNode {
                id: "c1".into(),
                name: cube_name.into(),
                source: CubeSource {
                    kind: "joined".into(),
                    table: Some(primary),
                    primary_key: def.primary_key.clone(),
                    ..Default::default()
                },
                x,
                y,
                ..Default::default()
            });
        }
    }
    def.primary_key = None;
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
    s.len() <= 63 && ident_pattern(s)
}

/// `^[A-Za-z_][A-Za-z0-9_]*$`, of any length: a cube name (Cube has no
/// cap, and a converted model's name may be long).
fn ident_pattern(s: &str) -> bool {
    let mut chars = s.chars();
    chars
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
/// (the editor saves while one is being placed); a joined cube does not read it. A
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

    validate_cubes(def, &aliases)
}

fn validate_cubes(def: &Definition, aliases: &HashSet<&str>) -> Result<(), String> {
    let mut seen = HashSet::new();
    let mut names = HashSet::new();
    for cube in &def.cubes {
        let name = cube.name.as_str();
        if !seen.insert(cube.id.as_str()) {
            return Err(format!("O objeto {} se repete", cube.id));
        }
        for size in [cube.width, cube.height].into_iter().flatten() {
            if !size.is_finite() || size <= 0.0 {
                return Err(format!("Tamanho inválido em {}: {size}", cube.id));
            }
        }
        if !cube.x.is_finite() || !cube.y.is_finite() {
            return Err(format!("Posição inválida em {}", cube.id));
        }
        if !ident_pattern(name) {
            return Err(format!("Nome de cubo inválido: {name}"));
        }
        if !names.insert(name) {
            return Err(format!("O cubo {name} se repete"));
        }
        if let Some(c) = &cube.color {
            if !COLORS.contains(&c.as_str()) {
                return Err(format!("Cor inválida: {c}"));
            }
        }
        if !SOURCE_KINDS.contains(&cube.source.kind.as_str()) {
            return Err(format!(
                "Origem inválida no cubo {name}: {}",
                cube.source.kind
            ));
        }
        for t in cube.source.table.iter().chain(&cube.source.tables) {
            if !aliases.contains(t.as_str()) {
                return Err(format!(
                    "O cubo {name} lê a tabela {t}, que não está no model"
                ));
            }
        }
        let mut members = HashSet::new();
        let member_names = cube
            .dimensions
            .iter()
            .map(|m| &m.name)
            .chain(cube.measures.iter().map(|m| &m.name))
            .chain(cube.segments.iter().map(|m| &m.name));
        for m in member_names {
            if !valid_ident(m) {
                return Err(format!("Nome de membro inválido: {name}.{m}"));
            }
            if !members.insert(m.as_str()) {
                return Err(format!("O membro {name}.{m} se repete"));
            }
        }
        let needs = |m: &str| format!("O membro {name}.{m} precisa de SQL ou de coluna, não dos dois");
        for d in &cube.dimensions {
            if !DIMENSION_TYPES.contains(&d.kind.as_str()) {
                return Err(format!("Tipo inválido em {name}.{}: {}", d.name, d.kind));
            }
            if d.sql.is_some() == d.column.is_some() {
                return Err(needs(&d.name));
            }
        }
        for m in &cube.measures {
            if !MEASURE_TYPES.contains(&m.kind.as_str()) {
                return Err(format!("Tipo inválido em {name}.{}: {}", m.name, m.kind));
            }
            if let Some(f) = &m.format {
                if !MEASURE_FORMATS.contains(&f.as_str()) {
                    return Err(format!("Tipo inválido em {name}.{}: {f}", m.name));
                }
            }
            let both = m.sql.is_some() && m.column.is_some();
            let neither = m.sql.is_none() && m.column.is_none() && m.kind != "count";
            if both || neither {
                return Err(needs(&m.name));
            }
        }
    }

    let ids: HashMap<&str, &str> = def
        .cubes
        .iter()
        .map(|c| (c.id.as_str(), c.name.as_str()))
        .collect();
    let mut seen = HashSet::new();
    let mut pairs: Vec<(&str, &str)> = Vec::new();
    for join in &def.cube_joins {
        if !seen.insert(join.id.as_str()) {
            return Err(format!("O objeto {} se repete", join.id));
        }
        let (Some(a), Some(b)) = (ids.get(join.from.as_str()), ids.get(join.to.as_str())) else {
            return Err(format!(
                "A ligação {} liga cubos que não estão no model",
                join.id
            ));
        };
        if join.from == join.to {
            return Err(format!(
                "A ligação {} liga cubos que não estão no model",
                join.id
            ));
        }
        if !RELATIONSHIPS.contains(&join.relationship.as_str()) {
            return Err(format!("Relacionamento inválido: {}", join.relationship));
        }
        if join.on.is_empty() {
            return Err(format!("A ligação {} não tem colunas", join.id));
        }
        if join.points.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
            return Err(format!("Ponto inválido na ligação {}", join.id));
        }
        let key = if join.from < join.to {
            (join.from.as_str(), join.to.as_str())
        } else {
            (join.to.as_str(), join.from.as_str())
        };
        if pairs.contains(&key) {
            return Err(format!("Os cubos {a} e {b} já estão ligados"));
        }
        pairs.push(key);
    }

    Ok(())
}

/// One cube's file, built by [`build_all`].
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltCube {
    pub cube_id: String,
    pub cube_name: String,
    /// The YAML, preceded by the header line.
    pub content: String,
}

/// A cube's members, before its joins are known.
struct Shaped {
    /// `sql` or `sql_table`, and its value.
    source: (&'static str, String),
    dimensions: Vec<Yaml>,
    measures: Vec<Yaml>,
    segments: Vec<Yaml>,
    /// What a cube join may name on this cube.
    dimension_names: HashSet<String>,
}

/// One YAML per cube of `def`, in `def.cubes` order, checked against the
/// connection's `columns` (keyed by plain schema name). A cube join is
/// written on its `from` cube.
pub fn build_all(
    name: &str,
    model_id: Uuid,
    def: &Definition,
    columns: &cubedriver::DatabaseStructure,
    db_type: &str,
) -> Result<Vec<BuiltCube>, String> {
    validate(def)?;
    let shaped = def
        .cubes
        .iter()
        .map(|cube| shape(def, cube, columns, db_type))
        .collect::<Result<Vec<_>, _>>()?;

    let index: HashMap<&str, usize> = def
        .cubes
        .iter()
        .enumerate()
        .map(|(i, c)| (c.id.as_str(), i))
        .collect();
    let mut joins: Vec<Vec<Yaml>> = vec![Vec::new(); def.cubes.len()];
    for join in &def.cube_joins {
        // validate: both ends are cubes of the model.
        let (from, to) = (index[join.from.as_str()], index[join.to.as_str()]);
        let to_name = def.cubes[to].name.as_str();
        let mut on = Vec::new();
        for pair in &join.on {
            for (i, m) in [(from, &pair.from), (to, &pair.to)] {
                if !shaped[i].dimension_names.contains(m.as_str()) {
                    return Err(format!(
                        "A ligação {} usa {}.{m}, que não é dimensão do cubo",
                        join.id, def.cubes[i].name
                    ));
                }
            }
            on.push(format!("{{CUBE.{}}} = {{{to_name}.{}}}", pair.from, pair.to));
        }
        let mut written = Mapping::new();
        written.insert("name".into(), to_name.into());
        written.insert("relationship".into(), join.relationship.as_str().into());
        written.insert("sql".into(), on.join(" AND ").into());
        joins[from].push(Yaml::Mapping(written));
    }

    def.cubes
        .iter()
        .zip(shaped)
        .zip(joins)
        .map(|((cube, shaped), joins)| {
            let mut out = Mapping::new();
            out.insert("name".into(), cube.name.as_str().into());
            if def.data_source != "default" {
                out.insert("data_source".into(), def.data_source.as_str().into());
            }
            out.insert(shaped.source.0.into(), shaped.source.1.into());
            for (key, text) in [("title", &cube.title), ("description", &cube.description)] {
                if let Some(text) = text.as_deref().filter(|t| !t.is_empty()) {
                    out.insert(key.into(), text.into());
                }
            }
            for (key, list) in [
                ("joins", joins),
                ("dimensions", shaped.dimensions),
                ("measures", shaped.measures),
                ("segments", shaped.segments),
            ] {
                if !list.is_empty() {
                    out.insert(key.into(), Yaml::Sequence(list));
                }
            }
            let mut doc = Mapping::new();
            doc.insert("cubes".into(), Yaml::Sequence(vec![Yaml::Mapping(out)]));
            let yaml = serde_yaml::to_string(&doc).map_err(|e| e.to_string())?;
            Ok(BuiltCube {
                cube_id: cube.id.clone(),
                cube_name: cube.name.clone(),
                content: format!(
                    "# Built from model {name} ({model_id}): Build cube rewrites this file\n{yaml}"
                ),
            })
        })
        .collect()
}

/// The columns of the tables a cube reads: a draft refuses the build, and a
/// table missing from the connection too.
fn read_tables<'a>(
    def: &Definition,
    tables: &[&'a ModelTable],
    columns: &'a cubedriver::DatabaseStructure,
) -> Result<Vec<&'a Vec<cubedriver::SchemaColumn>>, String> {
    if let Some(t) = tables.iter().find(|t| t.draft.is_some()) {
        return Err(format!(
            "A tabela {}.{} ainda não existe no banco: use Criar no banco",
            t.schema, t.table
        ));
    }
    tables
        .iter()
        .map(|table| {
            columns
                .get(&table.schema)
                .and_then(|t| t.get(&table.table))
                .ok_or_else(|| {
                    format!(
                        "A tabela {}.{} não existe na conexão {}",
                        table.schema, table.table, def.data_source
                    )
                })
        })
        .collect()
}

/// `base`, or `base_2`, `base_3`… when `taken` has it; the result is taken.
fn unique(taken: &mut BTreeSet<String>, base: String) -> String {
    let mut member = base.clone();
    let mut n = 2;
    while !taken.insert(member.clone()) {
        member = format!("{base}_{n}");
        n += 1;
    }
    member
}

fn auto_dimension(name: &str, sql: &str, kind: &str, primary_key: bool) -> Yaml {
    let mut dimension = Mapping::new();
    dimension.insert("name".into(), name.into());
    dimension.insert("sql".into(), sql.into());
    dimension.insert("type".into(), kind.into());
    if primary_key {
        dimension.insert("primary_key".into(), true.into());
    }
    Yaml::Mapping(dimension)
}

fn count() -> Yaml {
    let mut count = Mapping::new();
    count.insert("name".into(), "count".into());
    count.insert("type".into(), "count".into());
    Yaml::Mapping(count)
}

/// The dimensions, measures and segments a cube writes, with its own
/// members (explicit mode). `column` turns a member's column into its SQL.
fn explicit(
    cube: &CubeNode,
    column: &dyn Fn(&str) -> Result<String, String>,
) -> Result<Shaped, String> {
    let name = cube.name.as_str();
    // A member's SQL: its column's, else its own, which must not be blank
    // unless `optional` (a count), when a blank one is left out.
    let sql_of = |member: &str,
                  c: &Option<String>,
                  sql: &Option<String>,
                  optional: bool|
     -> Result<Option<String>, String> {
        match (c, sql) {
            (Some(c), _) => column(c).map(Some),
            (None, Some(sql)) if !sql.trim().is_empty() => Ok(Some(sql.clone())),
            (None, _) if optional => Ok(None),
            (None, _) => Err(format!("O membro {name}.{member} está sem SQL")),
        }
    };
    let text = |m: &mut Mapping, key: &str, v: &Option<String>| {
        if let Some(v) = v {
            m.insert(key.into(), v.as_str().into());
        }
    };
    let mut dimensions = Vec::new();
    for d in &cube.dimensions {
        let mut m = Mapping::new();
        m.insert("name".into(), d.name.as_str().into());
        text(&mut m, "sql", &sql_of(&d.name, &d.column, &d.sql, false)?);
        m.insert("type".into(), d.kind.as_str().into());
        if d.primary_key {
            m.insert("primary_key".into(), true.into());
        }
        text(&mut m, "title", &d.title);
        dimensions.push(Yaml::Mapping(m));
    }
    let mut measures = Vec::new();
    for x in &cube.measures {
        let mut m = Mapping::new();
        m.insert("name".into(), x.name.as_str().into());
        text(&mut m, "sql", &sql_of(&x.name, &x.column, &x.sql, x.kind == "count")?);
        m.insert("type".into(), x.kind.as_str().into());
        text(&mut m, "title", &x.title);
        text(&mut m, "format", &x.format);
        measures.push(Yaml::Mapping(m));
    }
    Ok(Shaped {
        source: ("", String::new()),
        dimensions,
        measures,
        segments: Vec::new(),
        dimension_names: cube.dimensions.iter().map(|d| d.name.clone()).collect(),
    })
}

fn segments(cube: &CubeNode) -> Result<Vec<Yaml>, String> {
    cube.segments
        .iter()
        .map(|s| {
            if s.sql.trim().is_empty() {
                return Err(format!("O segmento {}.{} está sem SQL", cube.name, s.name));
            }
            let mut m = Mapping::new();
            m.insert("name".into(), s.name.as_str().into());
            m.insert("sql".into(), s.sql.as_str().into());
            if let Some(title) = &s.title {
                m.insert("title".into(), title.as_str().into());
            }
            Ok(Yaml::Mapping(m))
        })
        .collect()
}

fn shape(
    def: &Definition,
    cube: &CubeNode,
    columns: &cubedriver::DatabaseStructure,
    db_type: &str,
) -> Result<Shaped, String> {
    let name = cube.name.as_str();
    let q = |ident: &str| quote(db_type, ident);
    let auto = cube.dimensions.is_empty() && cube.measures.is_empty();
    let missing = |c: &str| format!("A coluna {c} não existe no cubo {name}");
    // validate: a cube's tables are aliases of the model.
    let table_of = |alias: &str| def.tables.iter().find(|t| t.alias == alias).unwrap();
    let primary = || {
        cube.source
            .table
            .as_deref()
            .ok_or_else(|| format!("Escolha a tabela principal do cubo {name}"))
    };

    let mut shaped = match cube.source.kind.as_str() {
        "sql" => {
            if auto {
                return Err(format!("O cubo {name} não tem membros"));
            }
            let query = cube.source.sql.as_deref().unwrap_or("").trim();
            if query.is_empty() {
                return Err(format!("O cubo {name} está sem SQL"));
            }
            let mut shaped = explicit(cube, &|c| Ok(format!("{{CUBE}}.{}", q(c))))?;
            shaped.source = ("sql", query.to_string());
            shaped
        }
        "table" => {
            let alias = primary()?;
            let table = table_of(alias);
            let cols = read_tables(def, &[table], columns)?[0];
            let source = ("sql_table", format!("{}.{}", q(&table.schema), q(&table.table)));
            if auto {
                if let Some(key) = &cube.source.primary_key {
                    let column = cols
                        .iter()
                        .find(|c| &c.name == key)
                        .ok_or_else(|| format!("A coluna {alias}.{key} não existe"))?;
                    if cube_type(&column.type_).is_none() {
                        return Err(format!("A chave primária {key} não tem tipo no Cube"));
                    }
                }
                let mut taken = BTreeSet::from(["count".to_string()]);
                let mut dimensions = Vec::new();
                let mut dimension_names = HashSet::new();
                for column in cols {
                    let Some(kind) = cube_type(&column.type_) else {
                        continue;
                    };
                    let member = unique(&mut taken, snake(&column.name));
                    let is_key = cube.source.primary_key.as_deref() == Some(&column.name);
                    let sql = format!("{{CUBE}}.{}", q(&column.name));
                    dimensions.push(auto_dimension(&member, &sql, kind, is_key));
                    dimension_names.insert(member);
                }
                Shaped {
                    source,
                    dimensions,
                    measures: vec![count()],
                    segments: Vec::new(),
                    dimension_names,
                }
            } else {
                let mut shaped = explicit(cube, &|c| {
                    if cols.iter().any(|col| col.name == c) {
                        Ok(format!("{{CUBE}}.{}", q(c)))
                    } else {
                        Err(missing(c))
                    }
                })?;
                shaped.source = source;
                shaped
            }
        }
        _ => joined(def, cube, columns, db_type, primary()?, auto)?,
    };
    shaped.segments = segments(cube)?;
    Ok(shaped)
}

/// A join oriented from the primary: `from` is already in the tree.
struct Step<'a> {
    from: &'a End,
    to: &'a End,
    relationship: &'a str,
    join: &'a str,
}

/// A `joined` cube: the tables `def.joins` reach from `primary`, walked
/// breadth-first, in one SELECT of their typed columns.
fn joined(
    def: &Definition,
    cube: &CubeNode,
    columns: &cubedriver::DatabaseStructure,
    db_type: &str,
    primary: &str,
    auto: bool,
) -> Result<Shaped, String> {
    let name = cube.name.as_str();
    let primary_key = if auto {
        Some(
            cube.source
                .primary_key
                .as_deref()
                .ok_or_else(|| format!("Escolha a chave primária do cubo {name}"))?,
        )
    } else {
        None
    };

    // validate made the links a forest; a table this walk does not reach is
    // not part of the cube.
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

    let read: Vec<&ModelTable> = order
        .iter()
        .map(|alias| def.tables.iter().find(|t| t.alias == *alias).unwrap())
        .collect();
    let cols = read_tables(def, &read, columns)?;
    let tables: HashMap<&str, (&ModelTable, &Vec<cubedriver::SchemaColumn>)> = order
        .iter()
        .zip(read.iter().zip(cols))
        .map(|(alias, (table, cols))| (*alias, (*table, cols)))
        .collect();
    let column_of = |end: &End| {
        tables[end.alias.as_str()]
            .1
            .iter()
            .find(|c| c.name == end.column)
            .ok_or_else(|| format!("A coluna {}.{} não existe", end.alias, end.column))
    };
    for step in &steps {
        column_of(step.from)?;
        column_of(step.to)?;
    }
    if let Some(primary_key) = primary_key {
        let key = column_of(&End {
            alias: primary.into(),
            column: primary_key.into(),
        })?;
        if cube_type(&key.type_).is_none() {
            return Err(format!(
                "A chave primária {primary_key} não tem tipo no Cube"
            ));
        }
    }
    let multiplies = steps.iter().any(|s| s.relationship == "one_to_many");

    let q = |ident: &str| quote(db_type, ident);
    let mut taken = BTreeSet::from(["count".to_string()]);
    let mut select = Vec::new();
    // (alias, column) → its name in the SELECT.
    let mut selected: HashMap<(&str, &str), String> = HashMap::new();
    let mut dimensions = Vec::new();
    let mut key_sql = String::new();
    for alias in &order {
        let (_, cols) = tables[alias];
        for column in cols {
            let Some(kind) = cube_type(&column.type_) else {
                continue;
            };
            let member = unique(&mut taken, format!("{}_{}", snake(alias), snake(&column.name)));
            select.push(format!(
                "{}.{} AS {}",
                q(alias),
                q(&column.name),
                q(&member)
            ));
            let sql = format!("{{CUBE}}.{}", q(&member));
            let is_key = *alias == primary && primary_key == Some(column.name.as_str());
            if is_key {
                key_sql = sql.clone();
            }
            dimensions.push(auto_dimension(&member, &sql, kind, is_key));
            selected.insert((*alias, column.name.as_str()), member);
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
    let source = ("sql", sql.join("\n"));

    if !auto {
        let mut shaped = explicit(cube, &|c| {
            c.split_once('.')
                .and_then(|(alias, column)| selected.get(&(alias, column)))
                .map(|member| format!("{{CUBE}}.{}", q(member)))
                .ok_or_else(|| format!("A coluna {c} não existe no cubo {name}"))
        })?;
        shaped.source = source;
        return Ok(shaped);
    }

    let dimension_names = selected.into_values().collect();
    let mut count = Mapping::new();
    count.insert("name".into(), "count".into());
    if multiplies {
        // A one_to_many repeats the primary's rows: count its key, once.
        count.insert("type".into(), "count_distinct".into());
        count.insert("sql".into(), key_sql.into());
    } else {
        count.insert("type".into(), "count".into());
    }
    Ok(Shaped {
        source,
        dimensions,
        measures: vec![Yaml::Mapping(count)],
        segments: Vec::new(),
        dimension_names,
    })
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
        tables.insert(
            "payments".to_string(),
            vec![col("id", "int"), col("amount", "numeric")],
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

    fn build_def(def: &Definition) -> Result<Vec<BuiltCube>, String> {
        build_all("Sales", Uuid::nil(), def, &columns(), "postgres")
    }

    /// A converted model: `def` upgraded into cube `sales`, then built.
    fn run(def: &Definition) -> Result<String, String> {
        let mut def = def.clone();
        upgrade(&mut def, "sales");
        Ok(build_def(&def)?.remove(0).content)
    }

    fn cube_node(id: &str, name: &str, kind: &str, table: Option<&str>) -> CubeNode {
        CubeNode {
            id: id.into(),
            name: name.into(),
            source: CubeSource {
                kind: kind.into(),
                table: table.map(Into::into),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn dimension(name: &str, kind: &str, column: Option<&str>, sql: Option<&str>) -> CubeDimension {
        CubeDimension {
            name: name.into(),
            kind: kind.into(),
            column: column.map(Into::into),
            sql: sql.map(Into::into),
            ..Default::default()
        }
    }

    /// The YAML of the cube named `name` among `built`.
    fn yaml_of(built: &[BuiltCube], name: &str) -> Yaml {
        let b = built
            .iter()
            .find(|b| b.cube_name == name)
            .unwrap_or_else(|| panic!("no cube {name}"));
        cube(&b.content)
    }

    /// Cubes `orders` (c1) and `customers` (c2) on their tables, in auto
    /// mode, with cube join cj1 orders.customer_id → customers.id.
    fn two_cubes() -> Definition {
        let mut orders = cube_node("c1", "orders", "table", Some("orders"));
        orders.source.primary_key = Some("id".into());
        let mut customers = cube_node("c2", "customers", "table", Some("customers"));
        customers.source.primary_key = Some("id".into());
        Definition {
            data_source: "default".into(),
            tables: vec![table("orders", "orders"), table("customers", "customers")],
            cubes: vec![orders, customers],
            cube_joins: vec![CubeJoin {
                id: "cj1".into(),
                from: "c1".into(),
                to: "c2".into(),
                relationship: "many_to_one".into(),
                on: vec![ColumnPair {
                    from: "customer_id".into(),
                    to: "id".into(),
                }],
                ..Default::default()
            }],
            ..Default::default()
        }
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

    /// Today's build of [`orders_customers`], pinned before the build moved
    /// to cubes: a converted model must build these bytes.
    const GOLDEN_LEFT_JOIN: &str = "# Built from model Sales (00000000-0000-0000-0000-000000000000): Build cube rewrites this file\ncubes:\n- name: sales\n  sql: |-\n    SELECT \"orders\".\"id\" AS \"orders_id\", \"orders\".\"customer_id\" AS \"orders_customer_id\", \"orders\".\"Order ID\" AS \"orders_order_id\", \"orders\".\"order_id\" AS \"orders_order_id_2\", \"customers\".\"id\" AS \"customers_id\", \"customers\".\"region\" AS \"customers_region\"\n    FROM \"public\".\"orders\" AS \"orders\"\n    LEFT JOIN \"public\".\"customers\" AS \"customers\" ON \"orders\".\"customer_id\" = \"customers\".\"id\"\n  dimensions:\n  - name: orders_id\n    sql: '{CUBE}.\"orders_id\"'\n    type: number\n    primary_key: true\n  - name: orders_customer_id\n    sql: '{CUBE}.\"orders_customer_id\"'\n    type: number\n  - name: orders_order_id\n    sql: '{CUBE}.\"orders_order_id\"'\n    type: string\n  - name: orders_order_id_2\n    sql: '{CUBE}.\"orders_order_id_2\"'\n    type: number\n  - name: customers_id\n    sql: '{CUBE}.\"customers_id\"'\n    type: number\n  - name: customers_region\n    sql: '{CUBE}.\"customers_region\"'\n    type: string\n  measures:\n  - name: count\n    type: count\n";

    /// Today's build of [`one_to_many`].
    const GOLDEN_ONE_TO_MANY: &str = "# Built from model Sales (00000000-0000-0000-0000-000000000000): Build cube rewrites this file\ncubes:\n- name: sales\n  sql: |-\n    SELECT \"customers\".\"id\" AS \"customers_id\", \"customers\".\"region\" AS \"customers_region\", \"orders\".\"id\" AS \"orders_id\", \"orders\".\"customer_id\" AS \"orders_customer_id\", \"orders\".\"Order ID\" AS \"orders_order_id\", \"orders\".\"order_id\" AS \"orders_order_id_2\"\n    FROM \"public\".\"customers\" AS \"customers\"\n    LEFT JOIN \"public\".\"orders\" AS \"orders\" ON \"customers\".\"id\" = \"orders\".\"customer_id\"\n  dimensions:\n  - name: customers_id\n    sql: '{CUBE}.\"customers_id\"'\n    type: number\n    primary_key: true\n  - name: customers_region\n    sql: '{CUBE}.\"customers_region\"'\n    type: string\n  - name: orders_id\n    sql: '{CUBE}.\"orders_id\"'\n    type: number\n  - name: orders_customer_id\n    sql: '{CUBE}.\"orders_customer_id\"'\n    type: number\n  - name: orders_order_id\n    sql: '{CUBE}.\"orders_order_id\"'\n    type: string\n  - name: orders_order_id_2\n    sql: '{CUBE}.\"orders_order_id_2\"'\n    type: number\n  measures:\n  - name: count\n    type: count_distinct\n    sql: '{CUBE}.\"customers_id\"'\n";

    /// customers (primary, pk id) one_to_many orders, left.
    fn one_to_many() -> Definition {
        Definition {
            primary: Some("customers".into()),
            joins: vec![join(
                ("customers", "id"),
                ("orders", "customer_id"),
                "one_to_many",
                "left",
            )],
            ..orders_customers()
        }
    }

    #[test]
    fn converted_model_builds_the_golden_yaml() {
        for (def, golden) in [
            (orders_customers(), GOLDEN_LEFT_JOIN),
            (one_to_many(), GOLDEN_ONE_TO_MANY),
        ] {
            let mut def = def;
            upgrade(&mut def, "sales");
            let built = build_def(&def).unwrap();
            assert_eq!(built.len(), 1);
            assert_eq!(built[0].cube_id, "c1");
            assert_eq!(built[0].cube_name, "sales");
            assert_eq!(built[0].content, golden);
        }
    }

    #[test]
    fn table_cube_auto_members() {
        let built = build_def(&two_cubes()).unwrap();
        assert_eq!(
            built.iter().map(|b| b.cube_name.as_str()).collect::<Vec<_>>(),
            ["orders", "customers"]
        );
        let orders = yaml_of(&built, "orders");
        assert_eq!(orders["sql_table"].as_str(), Some(r#""public"."orders""#));
        assert!(orders["sql"].is_null());
        let names: Vec<_> = orders["dimensions"]
            .as_sequence()
            .unwrap()
            .iter()
            .map(|d| d["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["id", "customer_id", "order_id", "order_id_2"]);
        let id = member(&orders, "dimensions", "id");
        assert_eq!(id["primary_key"].as_bool(), Some(true));
        assert_eq!(id["sql"].as_str(), Some(r#"{CUBE}."id""#));
        assert_eq!(id["type"].as_str(), Some("number"));
        assert_eq!(
            member(&orders, "dimensions", "order_id")["sql"].as_str(),
            Some(r#"{CUBE}."Order ID""#)
        );
        assert_eq!(
            member(&orders, "dimensions", "customer_id")["primary_key"].as_bool(),
            None
        );
        assert_eq!(
            member(&orders, "measures", "count")["type"].as_str(),
            Some("count")
        );
    }

    #[test]
    fn sql_cube_needs_members() {
        let mut q = cube_node("c1", "q", "sql", None);
        q.source.sql = Some("SELECT 1 AS x".into());
        let def = Definition {
            data_source: "default".into(),
            cubes: vec![q],
            ..Default::default()
        };
        assert_eq!(
            build_def(&def).unwrap_err(),
            "O cubo q não tem membros"
        );
    }

    #[test]
    fn explicit_members_on_each_source() {
        let mut sales = cube_node("c1", "sales", "joined", Some("orders"));
        sales.dimensions = vec![dimension("region", "string", Some("customers.region"), None)];
        let mut payments = cube_node("c2", "payments", "table", Some("payments"));
        payments.title = Some("Pagamentos".into());
        payments.description = Some("Tudo que entrou".into());
        payments.dimensions = vec![CubeDimension {
            primary_key: true,
            title: Some("Código".into()),
            ..dimension("id", "number", Some("id"), None)
        }];
        payments.measures = vec![CubeMeasure {
            name: "total".into(),
            kind: "sum".into(),
            column: Some("amount".into()),
            format: Some("currency".into()),
            title: Some("Total".into()),
            ..Default::default()
        }];
        let mut q = cube_node("c3", "q", "sql", None);
        q.source.sql = Some("  SELECT 1 AS x\n".into());
        q.dimensions = vec![dimension("x", "number", None, Some("{CUBE}.x"))];
        q.segments = vec![CubeSegment {
            name: "positive".into(),
            sql: "{CUBE}.x > 0".into(),
            title: Some("Positivos".into()),
        }];
        let def = Definition {
            cubes: vec![sales, payments, q],
            tables: vec![
                table("orders", "orders"),
                table("customers", "customers"),
                table("payments", "payments"),
            ],
            ..orders_customers()
        };
        let mut def = def;
        def.primary = None;
        def.primary_key = None;
        let built = build_def(&def).unwrap();

        let sales = yaml_of(&built, "sales");
        assert!(sql(&sales).contains(r#"LEFT JOIN "public"."customers" AS "customers""#));
        let region = member(&sales, "dimensions", "region");
        assert_eq!(region["sql"].as_str(), Some(r#"{CUBE}."customers_region""#));
        assert_eq!(region["type"].as_str(), Some("string"));
        assert_eq!(sales["dimensions"].as_sequence().unwrap().len(), 1);
        assert!(sales["measures"].is_null(), "{sales:?}");

        let payments = yaml_of(&built, "payments");
        let keys: Vec<_> = payments
            .as_mapping()
            .unwrap()
            .keys()
            .map(|k| k.as_str().unwrap())
            .collect();
        assert_eq!(
            keys,
            ["name", "sql_table", "title", "description", "dimensions", "measures"]
        );
        assert_eq!(payments["title"].as_str(), Some("Pagamentos"));
        assert_eq!(payments["description"].as_str(), Some("Tudo que entrou"));
        let id = member(&payments, "dimensions", "id");
        assert_eq!(id["primary_key"].as_bool(), Some(true));
        assert_eq!(id["title"].as_str(), Some("Código"));
        let total = member(&payments, "measures", "total");
        assert_eq!(total["type"].as_str(), Some("sum"));
        assert_eq!(total["sql"].as_str(), Some(r#"{CUBE}."amount""#));
        assert_eq!(total["format"].as_str(), Some("currency"));
        assert_eq!(total["title"].as_str(), Some("Total"));

        let q = yaml_of(&built, "q");
        assert_eq!(q["sql"].as_str(), Some("SELECT 1 AS x"));
        assert_eq!(
            member(&q, "dimensions", "x")["sql"].as_str(),
            Some("{CUBE}.x")
        );
        let positive = member(&q, "segments", "positive");
        assert_eq!(positive["sql"].as_str(), Some("{CUBE}.x > 0"));
        assert_eq!(positive["title"].as_str(), Some("Positivos"));
        assert!(q["measures"].is_null());
    }

    /// A sql cube `q` (SELECT 1 AS x) with dimension `x`, measure `n`
    /// (sum of x) and segment `s`.
    fn sql_cube() -> Definition {
        let mut q = cube_node("c1", "q", "sql", None);
        q.source.sql = Some("SELECT 1 AS x".into());
        q.dimensions = vec![dimension("x", "number", None, Some("{CUBE}.x"))];
        q.measures = vec![CubeMeasure {
            name: "n".into(),
            kind: "sum".into(),
            sql: Some("{CUBE}.x".into()),
            ..Default::default()
        }];
        q.segments = vec![CubeSegment {
            name: "s".into(),
            sql: "{CUBE}.x > 0".into(),
            title: None,
        }];
        Definition {
            data_source: "default".into(),
            cubes: vec![q],
            ..Default::default()
        }
    }

    #[test]
    fn blank_sql_saves_but_does_not_build() {
        let blank = |edit: &dyn Fn(&mut CubeNode)| {
            let mut def = sql_cube();
            edit(&mut def.cubes[0]);
            validate(&def).unwrap();
            build_def(&def).unwrap_err()
        };
        assert_eq!(blank(&|c| c.dimensions[0].sql = Some("  ".into())), "O membro q.x está sem SQL");
        assert_eq!(blank(&|c| c.measures[0].sql = Some("".into())), "O membro q.n está sem SQL");
        assert_eq!(blank(&|c| c.segments[0].sql = " \n".into()), "O segmento q.s está sem SQL");
        assert_eq!(blank(&|c| c.source.sql = Some(" ".into())), "O cubo q está sem SQL");
        assert_eq!(blank(&|c| c.source.sql = None), "O cubo q está sem SQL");

        // A count needs no SQL: a blank one is left out.
        let mut def = sql_cube();
        def.cubes[0].measures[0] = CubeMeasure {
            name: "n".into(),
            kind: "count".into(),
            sql: Some(" ".into()),
            ..Default::default()
        };
        let built = build_def(&def).unwrap();
        let n = member(&yaml_of(&built, "q"), "measures", "n").clone();
        assert!(n["sql"].is_null(), "{n:?}");
        assert_eq!(n["type"].as_str(), Some("count"));
    }

    #[test]
    fn a_long_cube_name_is_valid() {
        let long = "a".repeat(80);
        let mut def = sql_cube();
        def.cubes[0].name = long.clone();
        validate(&def).unwrap();
        assert_eq!(build_def(&def).unwrap()[0].cube_name, long);
        def.cubes[0].name = format!("1{long}");
        assert_eq!(validate(&def).unwrap_err(), format!("Nome de cubo inválido: 1{long}"));
        // Member names keep the 63-character cap.
        let mut def = sql_cube();
        def.cubes[0].dimensions[0].name = "x".repeat(64);
        assert_eq!(validate(&def).unwrap_err(), format!("Nome de membro inválido: q.{}", "x".repeat(64)));
    }

    #[test]
    fn cube_join_on_the_from_cube() {
        let built = build_def(&two_cubes()).unwrap();
        let orders = yaml_of(&built, "orders");
        let expected: Yaml = serde_yaml::from_str(
            r#"[{name: customers, relationship: many_to_one, sql: "{CUBE.customer_id} = {customers.id}"}]"#,
        )
        .unwrap();
        assert_eq!(orders["joins"], expected);
        assert!(yaml_of(&built, "customers")["joins"].is_null());

        let mut two = two_cubes();
        two.cube_joins[0].on.push(ColumnPair {
            from: "id".into(),
            to: "id".into(),
        });
        let built = build_def(&two).unwrap();
        assert_eq!(
            yaml_of(&built, "orders")["joins"][0]["sql"].as_str(),
            Some("{CUBE.customer_id} = {customers.id} AND {CUBE.id} = {customers.id}")
        );
    }

    #[test]
    fn built_cubes_compile() {
        let sources: Vec<(String, String)> = build_def(&two_cubes())
            .unwrap()
            .into_iter()
            .map(|b| (format!("{}.yml", b.cube_name), b.content))
            .collect();
        let check = crate::rbac::model_validate::check(
            &sources,
            &cubemodel::TemplateContext::default(),
            None,
        );
        assert!(check.problems.is_empty(), "{:?}", check.problems);
    }

    #[test]
    fn unknown_join_member_is_refused() {
        let mut def = two_cubes();
        def.cube_joins[0].on[0].from = "nope".into();
        assert_eq!(
            build_def(&def).unwrap_err(),
            "A ligação cj1 usa orders.nope, que não é dimensão do cubo"
        );
        let mut def = two_cubes();
        def.cube_joins[0].on[0].to = "region_x".into();
        assert_eq!(
            build_def(&def).unwrap_err(),
            "A ligação cj1 usa customers.region_x, que não é dimensão do cubo"
        );
        // A measure is not a dimension.
        let mut def = two_cubes();
        def.cube_joins[0].on[0].to = "count".into();
        assert_eq!(
            build_def(&def).unwrap_err(),
            "A ligação cj1 usa customers.count, que não é dimensão do cubo"
        );
    }

    #[test]
    fn unknown_column_is_refused() {
        let mut def = two_cubes();
        def.cubes[0].dimensions = vec![dimension("x", "string", Some("nope"), None)];
        assert_eq!(
            build_def(&def).unwrap_err(),
            "A coluna nope não existe no cubo orders"
        );

        let mut def = orders_customers();
        upgrade(&mut def, "sales");
        def.cubes[0].measures = vec![CubeMeasure {
            name: "m".into(),
            kind: "max".into(),
            column: Some("customers.nope".into()),
            ..Default::default()
        }];
        assert_eq!(
            build_def(&def).unwrap_err(),
            "A coluna customers.nope não existe no cubo sales"
        );
        // Not a reached table either.
        def.cubes[0].measures[0].column = Some("region".into());
        assert_eq!(
            build_def(&def).unwrap_err(),
            "A coluna region não existe no cubo sales"
        );

        // Not checked for a sql cube.
        let mut q = cube_node("c1", "q", "sql", None);
        q.source.sql = Some("SELECT 1 AS x".into());
        q.dimensions = vec![dimension("x", "number", Some("whatever"), None)];
        let def = Definition {
            data_source: "default".into(),
            cubes: vec![q],
            ..Default::default()
        };
        let built = build_def(&def).unwrap();
        assert_eq!(
            member(&yaml_of(&built, "q"), "dimensions", "x")["sql"].as_str(),
            Some(r#"{CUBE}."whatever""#)
        );
    }

    #[test]
    fn a_draft_table_only_blocks_the_cubes_that_read_it() {
        let mut def = items_def();
        def.data_source = "default".into();
        def.tables.insert(0, table("orders", "orders"));
        let mut orders = cube_node("c1", "orders", "table", Some("orders"));
        orders.source.primary_key = Some("id".into());
        def.cubes = vec![orders];
        assert_eq!(build_def(&def).unwrap().len(), 1);

        // A joined cube that does not reach the draft builds too.
        def.cubes[0].source.kind = "joined".into();
        assert_eq!(build_def(&def).unwrap().len(), 1);

        def.cubes.push(cube_node("c2", "items", "table", Some("i")));
        assert_eq!(
            build_def(&def).unwrap_err(),
            "A tabela public.items ainda não existe no banco: use Criar no banco"
        );
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
        upgrade(&mut no_primary, "sales");
        no_primary.cubes[0].source.table = None;
        assert_eq!(
            build_def(&no_primary).unwrap_err(),
            "Escolha a tabela principal do cubo sales"
        );

        let mut no_key = orders_customers();
        no_key.primary_key = None;
        assert_eq!(
            run(&no_key),
            Err("Escolha a chave primária do cubo sales".into())
        );

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

        // A table the walk does not reach is not part of the cube.
        let mut unlinked = orders_customers();
        unlinked.tables.push(table("x", "gone"));
        assert_eq!(run(&unlinked), Ok(GOLDEN_LEFT_JOIN.to_string()));

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
        upgrade(&mut def, "sales");
        assert_eq!(
            build_all("Sales", Uuid::nil(), &def, &db, "postgres").unwrap_err(),
            "A chave primária tags não tem tipo no Cube"
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
        def.joins = vec![join(("o", "id"), ("i", "id"), "many_to_one", "left")];
        upgrade(&mut def, "m");
        let err = build_all("m", Uuid::nil(), &def, &columns(), "postgres").unwrap_err();
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

    fn cubes_json() -> serde_json::Value {
        let mut v = v2_json();
        v["cubes"] = serde_json::json!([{
            "id": "c1", "name": "sales", "title": "Sales", "description": "d", "color": "teal",
            "source": { "kind": "joined", "table": "orders", "primaryKey": "id" },
            "dimensions": [
                { "name": "status", "type": "string", "column": "orders.status", "title": "Status" },
                { "name": "id", "type": "number", "sql": "orders.id", "primaryKey": true }
            ],
            "measures": [
                { "name": "count", "type": "count" },
                { "name": "total", "type": "sum", "column": "orders.total", "title": "T", "format": "currency" }
            ],
            "segments": [{ "name": "big", "sql": "total > 10", "title": "Big" }],
            "x": 10.0, "y": 20.0, "width": 240.0, "height": 180.0, "z": 4
        }, {
            "id": "c2", "name": "recent", 
            "source": { "kind": "sql", "sql": "select 1", "tables": ["customers"] },
            "dimensions": [], "measures": [], "segments": [],
            "x": 400.0, "y": 20.0
        }]);
        v["cubeJoins"] = serde_json::json!([{
            "id": "cj1", "from": "c1", "to": "c2", "relationship": "many_to_one",
            "on": [{ "from": "status", "to": "id" }],
            "points": [{ "x": 1.0, "y": 2.0 }], "hidden": true
        }]);
        v
    }

    #[test]
    fn accepts_cubes_and_cube_joins() {
        let value = cubes_json();
        let def: Definition = serde_json::from_value(value.clone()).unwrap();
        validate(&def).unwrap();
        assert_eq!(serde_json::to_value(&def).unwrap(), value);
        let old = serde_json::to_value(orders_customers()).unwrap();
        let keys = old.as_object().unwrap();
        for k in ["cubes", "cubeJoins"] {
            assert!(!keys.contains_key(k), "{k}");
        }
    }

    #[test]
    fn upgrade_turns_primary_into_a_cube() {
        let mut def: Definition = serde_json::from_value(v2_json()).unwrap();
        upgrade(&mut def, "vendas");
        assert_eq!(
            def.cubes,
            vec![CubeNode {
                id: "c1".into(),
                name: "vendas".into(),
                source: CubeSource {
                    kind: "joined".into(),
                    table: Some("orders".into()),
                    primary_key: Some("id".into()),
                    ..Default::default()
                },
                x: 620.0,
                y: 0.0,
                ..Default::default()
            }]
        );
        assert_eq!((def.primary.clone(), def.primary_key.clone()), (None, None));
        let once = def.clone();
        upgrade(&mut def, "vendas");
        assert_eq!(def, once);
        let mut none = orders_customers();
        none.primary = None;
        upgrade(&mut none, "vendas");
        assert!(none.cubes.is_empty());
    }

    #[test]
    fn refuses_bad_cubes() {
        let bad = |edit: &dyn Fn(&mut serde_json::Value)| {
            let mut value = cubes_json();
            edit(&mut value);
            let def: Definition = serde_json::from_value(value).unwrap();
            validate(&def).unwrap_err()
        };
        assert_eq!(bad(&|v| v["cubes"][1]["id"] = "c1".into()), "O objeto c1 se repete");
        assert_eq!(bad(&|v| v["cubes"][0]["name"] = "1x".into()), "Nome de cubo inválido: 1x");
        assert_eq!(bad(&|v| v["cubes"][1]["name"] = "sales".into()), "O cubo sales se repete");
        assert_eq!(
            bad(&|v| v["cubes"][0]["source"]["kind"] = "view".into()),
            "Origem inválida no cubo sales: view"
        );
        assert_eq!(
            bad(&|v| v["cubes"][0]["source"]["table"] = "ghost".into()),
            "O cubo sales lê a tabela ghost, que não está no model"
        );
        assert_eq!(
            bad(&|v| v["cubes"][1]["source"]["tables"] = serde_json::json!(["ghost"])),
            "O cubo recent lê a tabela ghost, que não está no model"
        );
        assert_eq!(
            bad(&|v| v["cubes"][0]["dimensions"][0]["name"] = "a-b".into()),
            "Nome de membro inválido: sales.a-b"
        );
        assert_eq!(
            bad(&|v| v["cubes"][0]["measures"][1]["name"] = "status".into()),
            "O membro sales.status se repete"
        );
        assert_eq!(
            bad(&|v| v["cubes"][0]["dimensions"][0]["type"] = "int".into()),
            "Tipo inválido em sales.status: int"
        );
        assert_eq!(
            bad(&|v| v["cubes"][0]["measures"][1]["type"] = "median".into()),
            "Tipo inválido em sales.total: median"
        );
        assert_eq!(
            bad(&|v| v["cubes"][0]["measures"][1]["format"] = "euro".into()),
            "Tipo inválido em sales.total: euro"
        );
        let both = "O membro sales.status precisa de SQL ou de coluna, não dos dois";
        assert_eq!(bad(&|v| v["cubes"][0]["dimensions"][0]["sql"] = "x".into()), both);
        assert_eq!(
            bad(&|v| v["cubes"][0]["dimensions"][0].as_object_mut().unwrap().remove("column")
                .map(|_| ()).unwrap_or(())),
            both
        );
        assert_eq!(
            bad(&|v| v["cubes"][0]["measures"][1].as_object_mut().unwrap().remove("column")
                .map(|_| ()).unwrap_or(())),
            "O membro sales.total precisa de SQL ou de coluna, não dos dois"
        );
        assert_eq!(bad(&|v| v["cubes"][0]["color"] = "nope".into()), "Cor inválida: nope");
    }

    #[test]
    fn refuses_bad_cube_joins() {
        let bad = |edit: &dyn Fn(&mut serde_json::Value)| {
            let mut value = cubes_json();
            edit(&mut value);
            let def: Definition = serde_json::from_value(value).unwrap();
            validate(&def).unwrap_err()
        };
        let msg = "A ligação cj1 liga cubos que não estão no model";
        assert_eq!(bad(&|v| v["cubeJoins"][0]["to"] = "c9".into()), msg);
        assert_eq!(bad(&|v| v["cubeJoins"][0]["to"] = "c1".into()), msg);
        assert_eq!(
            bad(&|v| v["cubeJoins"][0]["relationship"] = "many".into()),
            "Relacionamento inválido: many"
        );
        assert_eq!(
            bad(&|v| v["cubeJoins"][0]["on"] = serde_json::json!([])),
            "A ligação cj1 não tem colunas"
        );
        assert_eq!(
            bad(&|v| v["cubeJoins"].as_array_mut().unwrap().push(serde_json::json!({
                "id": "cj2", "from": "c2", "to": "c1", "relationship": "one_to_one",
                "on": [{ "from": "id", "to": "id" }], "points": []
            }))),
            "Os cubos recent e sales já estão ligados"
        );
    }
}
