//! Typed representation of a Cube data model.
//!
//! Every property present in the YAML survives into these structs: the ones the
//! rest of the crate needs are typed, the remainder lands in the `extra` map of
//! the owning struct. Raw `sql` / `sql_table` expressions are kept verbatim as
//! strings — this crate never evaluates them.

use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// Anything that is keyed by member name keeps YAML declaration order.
pub type Members<T> = IndexMap<String, T>;
/// Unrecognised properties, preserved verbatim.
pub type Extra = IndexMap<String, Value>;

fn de_opt_string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let value = Option::<Value>::deserialize(d)?;
    Ok(match value {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s),
        Some(Value::Bool(b)) => Some(b.to_string()),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(other) => Some(other.to_string()),
    })
}

/// Lenient list of arbitrary values: a bare scalar is wrapped into a one-element
/// list so a malformed document still parses and gets reported by the validator
/// rather than aborting the whole cube.
fn de_value_list<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Value>, D::Error> {
    let value = Option::<Value>::deserialize(d)?;
    Ok(match value {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items,
        Some(other) => vec![other],
    })
}

fn de_string_list<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    let value = Option::<Value>::deserialize(d)?;
    Ok(match value {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(s)) => vec![s],
        Some(Value::Array(items)) => items
            .into_iter()
            .map(|i| match i {
                Value::String(s) => s,
                other => other.to_string(),
            })
            .collect(),
        Some(other) => vec![other.to_string()],
    })
}

/// A whole model directory: cubes and views, each in file / declaration order.
#[derive(Debug, Clone, Default)]
pub struct DataModel {
    pub cubes: Members<CubeDef>,
    pub views: Members<CubeDef>,
}

impl DataModel {
    /// Cubes first, then views — the order `CubeSymbols.cubeList` produces.
    pub fn cube_list(&self) -> Vec<&CubeDef> {
        self.cubes.values().chain(self.views.values()).collect()
    }

    pub fn get(&self, name: &str) -> Option<&CubeDef> {
        self.cubes.get(name).or_else(|| self.views.get(name))
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut CubeDef> {
        if self.cubes.contains_key(name) {
            self.cubes.get_mut(name)
        } else {
            self.views.get_mut(name)
        }
    }

    pub fn contains(&self, name: &str) -> bool {
        self.cubes.contains_key(name) || self.views.contains_key(name)
    }

    pub fn names(&self) -> Vec<String> {
        self.cubes
            .keys()
            .chain(self.views.keys())
            .cloned()
            .collect()
    }
}

/// A cube or a view. Views set `is_view`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CubeDef {
    pub name: String,
    #[serde(skip)]
    pub file_name: String,
    #[serde(skip)]
    pub is_view: bool,

    #[serde(deserialize_with = "de_opt_string")]
    pub extends: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub sql: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub sql_table: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub sql_alias: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub data_source: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub title: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub description: Option<String>,
    pub public: Option<bool>,
    pub shown: Option<bool>,
    pub visible: Option<bool>,
    pub rewrite_queries: Option<bool>,
    pub calendar: Option<bool>,
    pub refresh_key: Option<Value>,
    pub meta: Option<Value>,

    pub measures: Members<Measure>,
    pub dimensions: Members<Dimension>,
    pub segments: Members<Segment>,
    pub hierarchies: Members<Hierarchy>,
    pub pre_aggregations: Members<PreAggregation>,
    pub joins: Vec<Join>,
    /// Kept as raw JSON: access policies are consumed by the query layer, not here.
    #[serde(deserialize_with = "de_value_list")]
    pub access_policy: Vec<Value>,

    // View-only
    pub cubes: Vec<ViewCubeInclude>,
    pub folders: Vec<Folder>,
    #[serde(deserialize_with = "de_string_list")]
    pub view_groups: Vec<String>,
    #[serde(deserialize_with = "de_value_list")]
    pub default_filters: Vec<Value>,

    #[serde(flatten)]
    pub extra: Extra,

    // ---- Derived by view resolution; never deserialized. ----
    #[serde(skip)]
    pub included_members: Vec<IncludedMember>,
    #[serde(skip)]
    pub evaluated_hierarchies: Vec<EvaluatedHierarchy>,
    #[serde(skip)]
    pub evaluated_folders: Vec<EvaluatedFolder>,
    #[serde(skip)]
    pub join_map: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Measure {
    #[serde(deserialize_with = "de_opt_string")]
    pub sql: Option<String>,
    #[serde(rename = "type", deserialize_with = "de_opt_string")]
    pub member_type: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub title: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub description: Option<String>,
    pub format: Option<Value>,
    #[serde(deserialize_with = "de_opt_string")]
    pub currency: Option<String>,
    pub meta: Option<Value>,
    pub public: Option<bool>,
    pub shown: Option<bool>,
    pub visible: Option<bool>,
    pub cumulative: Option<bool>,
    #[serde(deserialize_with = "de_opt_string")]
    pub agg_type: Option<String>,
    #[serde(deserialize_with = "de_string_list")]
    pub drill_members: Vec<String>,
    #[serde(deserialize_with = "de_string_list")]
    pub drill_member_references: Vec<String>,
    #[serde(deserialize_with = "de_string_list")]
    pub aliases: Vec<String>,
    #[serde(deserialize_with = "de_value_list")]
    pub filters: Vec<Value>,
    #[serde(deserialize_with = "de_value_list")]
    pub drill_filters: Vec<Value>,
    pub rolling_window: Option<Value>,
    pub multi_stage: Option<bool>,
    #[serde(deserialize_with = "de_value_list")]
    pub time_shift: Vec<Value>,
    #[serde(deserialize_with = "de_value_list")]
    pub order_by: Vec<Value>,
    pub mask: Option<Value>,
    #[serde(deserialize_with = "de_opt_string")]
    pub alias_member: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Dimension {
    #[serde(deserialize_with = "de_opt_string")]
    pub sql: Option<String>,
    #[serde(rename = "type", deserialize_with = "de_opt_string")]
    pub member_type: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub title: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub description: Option<String>,
    pub format: Option<Value>,
    #[serde(deserialize_with = "de_opt_string")]
    pub currency: Option<String>,
    pub meta: Option<Value>,
    pub public: Option<bool>,
    pub shown: Option<bool>,
    pub visible: Option<bool>,
    pub primary_key: Option<bool>,
    pub sub_query: Option<bool>,
    pub propagate_filters_to_sub_query: Option<bool>,
    pub suggest_filter_values: Option<bool>,
    pub values_as_segments: Option<bool>,
    pub enable_suggestions: Option<bool>,
    #[serde(deserialize_with = "de_opt_string")]
    pub order: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub key_reference: Option<String>,
    #[serde(deserialize_with = "de_string_list")]
    pub values: Vec<String>,
    #[serde(deserialize_with = "de_string_list")]
    pub aliases: Vec<String>,
    pub granularities: Members<Granularity>,
    pub case: Option<Value>,
    pub latitude: Option<Value>,
    pub longitude: Option<Value>,
    pub multi_stage: Option<bool>,
    pub mask: Option<Value>,
    pub links: Option<Value>,
    pub synthetic: Option<bool>,
    #[serde(deserialize_with = "de_value_list")]
    pub time_shift: Vec<Value>,
    #[serde(deserialize_with = "de_opt_string")]
    pub alias_member: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Granularity {
    #[serde(deserialize_with = "de_opt_string")]
    pub title: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub interval: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub offset: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub origin: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub sql: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Segment {
    #[serde(deserialize_with = "de_opt_string")]
    pub sql: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub title: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub description: Option<String>,
    pub meta: Option<Value>,
    pub public: Option<bool>,
    pub shown: Option<bool>,
    pub visible: Option<bool>,
    #[serde(deserialize_with = "de_string_list")]
    pub aliases: Vec<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Hierarchy {
    #[serde(deserialize_with = "de_opt_string")]
    pub title: Option<String>,
    pub public: Option<bool>,
    #[serde(deserialize_with = "de_string_list")]
    pub levels: Vec<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Join {
    pub name: String,
    #[serde(deserialize_with = "de_opt_string")]
    pub sql: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub relationship: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PreAggregation {
    #[serde(rename = "type", deserialize_with = "de_opt_string")]
    pub pre_agg_type: Option<String>,
    #[serde(deserialize_with = "de_string_list")]
    pub measures: Vec<String>,
    #[serde(deserialize_with = "de_string_list")]
    pub dimensions: Vec<String>,
    #[serde(deserialize_with = "de_string_list")]
    pub segments: Vec<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub time_dimension: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub time_dimension_reference: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub granularity: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub partition_granularity: Option<String>,
    #[serde(deserialize_with = "de_string_list")]
    pub rollups: Vec<String>,
    #[serde(deserialize_with = "de_string_list")]
    pub unique_key_columns: Vec<String>,
    #[serde(deserialize_with = "de_value_list")]
    pub time_dimensions: Vec<Value>,
    #[serde(deserialize_with = "de_string_list")]
    pub rollup_references: Vec<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub sql: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub sql_alias: Option<String>,
    pub refresh_key: Option<Value>,
    pub indexes: Members<Value>,
    pub external: Option<bool>,
    pub scheduled_refresh: Option<bool>,
    pub use_original_sql_pre_aggregations: Option<bool>,
    pub read_only: Option<bool>,
    pub union_with_source_data: Option<bool>,
    pub build_range_start: Option<Value>,
    pub build_range_end: Option<Value>,
    pub allow_non_strict_date_range_match: Option<bool>,
    #[serde(deserialize_with = "de_opt_string")]
    pub stream_offset: Option<String>,
    pub max_pre_aggregations: Option<Value>,
    #[serde(deserialize_with = "de_value_list")]
    pub output_column_types: Vec<Value>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// One entry of a view's `cubes:` list.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ViewCubeInclude {
    #[serde(deserialize_with = "de_opt_string")]
    pub join_path: Option<String>,
    pub prefix: Option<bool>,
    pub split: Option<bool>,
    #[serde(deserialize_with = "de_opt_string")]
    pub alias: Option<String>,
    pub includes: Includes,
    #[serde(deserialize_with = "de_string_list")]
    pub excludes: Vec<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `includes: "*"` or an explicit list.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Includes {
    #[default]
    None,
    All(StarMarker),
    List(Vec<IncludeItem>),
}

/// The literal `"*"`.
#[derive(Debug, Clone, Serialize)]
pub struct StarMarker;

impl<'de> Deserialize<'de> for StarMarker {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        if s == "*" {
            Ok(StarMarker)
        } else {
            Err(serde::de::Error::custom("expected \"*\""))
        }
    }
}

/// A view include entry: a bare member name or a member with overrides.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum IncludeItem {
    Name(String),
    Detailed(Box<IncludeMember>),
}

impl IncludeItem {
    pub fn name(&self) -> &str {
        match self {
            IncludeItem::Name(n) => n,
            IncludeItem::Detailed(m) => &m.name,
        }
    }

    pub fn alias(&self) -> Option<&str> {
        match self {
            IncludeItem::Name(_) => None,
            IncludeItem::Detailed(m) => m.alias.as_deref(),
        }
    }

    pub fn overrides(&self) -> Option<&IncludeMember> {
        match self {
            IncludeItem::Name(_) => None,
            IncludeItem::Detailed(m) => Some(m),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct IncludeMember {
    pub name: String,
    #[serde(deserialize_with = "de_opt_string")]
    pub alias: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub title: Option<String>,
    #[serde(deserialize_with = "de_opt_string")]
    pub description: Option<String>,
    pub format: Option<Value>,
    #[serde(deserialize_with = "de_opt_string")]
    pub currency: Option<String>,
    pub meta: Option<Value>,
    #[serde(flatten)]
    pub extra: Extra,
}

impl IncludeMember {
    pub fn has_overrides(&self) -> bool {
        self.title.is_some()
            || self.description.is_some()
            || self.format.is_some()
            || self.meta.is_some()
            || self.currency.is_some()
    }
}

/// A view folder as authored.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Folder {
    pub name: String,
    pub includes: FolderIncludes,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(untagged)]
pub enum FolderIncludes {
    #[default]
    None,
    All(StarMarker),
    List(Vec<FolderInclude>),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum FolderInclude {
    Member(String),
    JoinPath(JoinPathRef),
    Nested(Box<Folder>),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinPathRef {
    #[serde(deserialize_with = "de_opt_string")]
    pub join_path: Option<String>,
}

/// One flattened member of a view, as `CubeSymbols.prepareIncludes` records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncludedMember {
    /// `measures` | `dimensions` | `segments` | `hierarchies`
    pub member_type: String,
    /// `<sourceCube>.<memberName>`
    pub member_path: String,
    /// Name inside the view (prefixed / aliased).
    pub name: String,
}

/// A hierarchy after evaluation, with levels resolved to full member paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluatedHierarchy {
    pub name: String,
    pub title: Option<String>,
    pub public: Option<bool>,
    pub levels: Vec<String>,
    pub alias_member: Option<String>,
}

/// A folder after evaluation; nested folders are preserved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluatedFolder {
    pub name: String,
    pub includes: Vec<EvaluatedFolderItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvaluatedFolderItem {
    Member(IncludedMember),
    Folder(EvaluatedFolder),
}
