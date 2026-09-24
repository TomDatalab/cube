//! The input to the schema builder and the translator: Cube's meta config.
//!
//! Two JSON shapes are accepted, so the same code serves both the Rust and the
//! legacy Node.js side:
//!
//! * `{ "cubes": [ { "name": "Orders", "measures": [...], ... } ] }` — what
//!   `cubemodel::rest_meta_response` produces (and what `/v1/meta` returns);
//! * `[ { "config": { "name": "Orders", ... } } ]` — the `metaConfig` array the
//!   Node.js `CompilerApi.metaConfig()` hands to `makeSchema`.
//!
//! Both are also accepted nested, i.e. `{ "cubes": [ { "config": { ... } } ] }`.

use serde_json::Value;

use crate::error::GraphQLError;
use crate::naming::capitalize;

/// `MemberType` from `src/types/enums.ts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberType {
    /// A measure — goes into `query.measures`.
    Measures,
    /// A dimension — goes into `query.dimensions` or `query.timeDimensions`.
    Dimensions,
}

impl MemberType {
    /// The JSON key the member lives under in the meta config.
    pub fn as_str(&self) -> &'static str {
        match self {
            MemberType::Measures => "measures",
            MemberType::Dimensions => "dimensions",
        }
    }
}

/// One measure or dimension of a cube.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberMeta {
    /// Fully qualified name, e.g. `Orders.createdAt`.
    pub name: String,
    /// `string` / `number` / `time` / ... Missing types map to `String`, as in
    /// `graphql.ts`'s `mapType` default branch.
    pub member_type: Option<String>,
    /// Rendered as the GraphQL field description.
    pub description: Option<String>,
    /// Hidden members are left out of the schema entirely.
    pub is_visible: bool,
}

impl MemberMeta {
    /// The GraphQL field name — the member name without its cube prefix.
    pub fn field_name(&self) -> String {
        crate::naming::safe_name(&self.name)
    }
}

/// One cube (or view) of the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CubeMeta {
    /// The cube name as the model spells it, e.g. `Orders` or `orders`.
    pub name: String,
    /// `public: false` removes the whole cube from the schema.
    pub public: bool,
    /// Visible and hidden measures, in model order.
    pub measures: Vec<MemberMeta>,
    /// Visible and hidden dimensions, in model order.
    pub dimensions: Vec<MemberMeta>,
}

impl CubeMeta {
    /// `hasMembers` from `graphql.ts`: a cube makes it into the schema only
    /// when it is public and has at least one visible member.
    pub fn has_members(&self) -> bool {
        if !self.public {
            return false;
        }

        self.measures
            .iter()
            .chain(self.dimensions.iter())
            .any(|m| m.is_visible)
    }

    /// Visible measures, in model order.
    pub fn visible_measures(&self) -> impl Iterator<Item = &MemberMeta> {
        self.measures.iter().filter(|m| m.is_visible)
    }

    /// Visible dimensions, in model order.
    pub fn visible_dimensions(&self) -> impl Iterator<Item = &MemberMeta> {
        self.dimensions.iter().filter(|m| m.is_visible)
    }

    /// Every visible member, measures first — the order `graphql.ts` emits
    /// fields in.
    pub fn visible_members(&self) -> impl Iterator<Item = &MemberMeta> {
        self.visible_measures().chain(self.visible_dimensions())
    }
}

/// The parsed meta config.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetaConfig {
    /// Every cube of the model, in model order.
    pub cubes: Vec<CubeMeta>,
}

impl MetaConfig {
    /// Parse `{ "cubes": [...] }` (or a bare array of cube configs).
    pub fn from_value(value: &Value) -> Result<Self, GraphQLError> {
        let entries: &Vec<Value> = match value {
            Value::Array(items) => items,
            Value::Object(obj) => obj.get("cubes").and_then(Value::as_array).ok_or_else(|| {
                GraphQLError::Meta(
                    "Invalid meta config: expected an object with a `cubes` array".to_string(),
                )
            })?,
            _ => {
                return Err(GraphQLError::Meta(
                    "Invalid meta config: expected an object with a `cubes` array".to_string(),
                ))
            }
        };

        let cubes = entries.iter().filter_map(parse_cube).collect();

        Ok(MetaConfig { cubes })
    }

    /// `metaConfig.find(cube => cube.config.name === name)` — an exact,
    /// case-sensitive lookup. `graphql.ts` uses it to decide whether a name
    /// from the document needs capitalizing.
    pub fn find_exact(&self, name: &str) -> Option<&CubeMeta> {
        self.cubes.iter().find(|c| c.name == name)
    }

    /// The lookup `getMemberType` performs: match the cube by its literal name
    /// or by its capitalized name.
    pub fn find_cube(&self, name: &str) -> Option<&CubeMeta> {
        let capitalized = capitalize(name);
        self.cubes
            .iter()
            .find(|c| c.name == name || c.name == capitalized)
    }

    /// `getMemberType(metaConfig, cubeName, memberName)`.
    pub fn member_type(&self, cube_name: &str, member_name: &str) -> Option<MemberType> {
        let cube = self.find_cube(cube_name)?;
        let key = format!("{cube_name}.{member_name}");
        let capitalized_key = format!("{}.{}", capitalize(cube_name), member_name);

        let matches = |members: &[MemberMeta]| {
            members
                .iter()
                .any(|m| m.name == key || m.name == capitalized_key)
        };

        if matches(&cube.measures) {
            Some(MemberType::Measures)
        } else if matches(&cube.dimensions) {
            Some(MemberType::Dimensions)
        } else {
            None
        }
    }

    /// Every cube that makes it into the generated schema.
    pub fn schema_cubes(&self) -> impl Iterator<Item = &CubeMeta> {
        self.cubes.iter().filter(|c| c.has_members())
    }
}

fn parse_cube(entry: &Value) -> Option<CubeMeta> {
    let outer = entry.as_object()?;
    // `{ config: {...} }` (Node `metaConfig`) or the flattened REST shape.
    let config = match outer.get("config") {
        Some(Value::Object(inner)) => inner,
        _ => outer,
    };

    let name = config.get("name").and_then(Value::as_str)?.to_string();

    // `cube.public === false` is checked on the wrapper in `graphql.ts`; the
    // REST shape carries it on the config itself.
    let public = outer
        .get("public")
        .or_else(|| config.get("public"))
        .and_then(Value::as_bool)
        .unwrap_or(true);

    Some(CubeMeta {
        name,
        public,
        measures: parse_members(config.get("measures")),
        dimensions: parse_members(config.get("dimensions")),
    })
}

fn parse_members(value: Option<&Value>) -> Vec<MemberMeta> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let obj = item.as_object()?;
                    Some(MemberMeta {
                        name: obj.get("name").and_then(Value::as_str)?.to_string(),
                        member_type: obj.get("type").and_then(Value::as_str).map(str::to_string),
                        description: obj
                            .get("description")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        is_visible: obj
                            .get("isVisible")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn meta() -> MetaConfig {
        MetaConfig::from_value(&json!({
            "cubes": [{
                "name": "Orders",
                "measures": [{ "name": "Orders.count", "isVisible": true }],
                "dimensions": [
                    { "name": "Orders.status", "type": "string", "isVisible": true },
                    { "name": "Orders.secret", "type": "string", "isVisible": false }
                ]
            }]
        }))
        .unwrap()
    }

    #[test]
    fn parses_the_rest_meta_shape() {
        let meta = meta();
        assert_eq!(meta.cubes.len(), 1);
        assert_eq!(meta.cubes[0].name, "Orders");
        assert!(meta.cubes[0].has_members());
        assert_eq!(meta.cubes[0].visible_dimensions().count(), 1);
    }

    #[test]
    fn parses_the_node_meta_config_shape() {
        let meta = MetaConfig::from_value(&json!([{
            "config": {
                "name": "Orders",
                "measures": [{ "name": "Orders.count", "isVisible": true }],
                "dimensions": []
            }
        }]))
        .unwrap();
        assert_eq!(meta.cubes[0].name, "Orders");
    }

    #[test]
    fn member_type_tolerates_uncapitalized_cube_names() {
        let meta = meta();
        assert_eq!(
            meta.member_type("orders", "count"),
            Some(MemberType::Measures)
        );
        assert_eq!(
            meta.member_type("Orders", "status"),
            Some(MemberType::Dimensions)
        );
        assert_eq!(meta.member_type("Orders", "nope"), None);
        assert_eq!(meta.member_type("Nope", "count"), None);
    }

    #[test]
    fn non_public_cubes_are_dropped_from_the_schema() {
        let meta = MetaConfig::from_value(&json!({
            "cubes": [{
                "name": "Orders",
                "public": false,
                "measures": [{ "name": "Orders.count", "isVisible": true }],
                "dimensions": []
            }]
        }))
        .unwrap();
        assert_eq!(meta.schema_cubes().count(), 0);
    }

    #[test]
    fn rejects_garbage() {
        assert!(MetaConfig::from_value(&json!({ "nope": 1 })).is_err());
        assert!(MetaConfig::from_value(&json!("nope")).is_err());
    }
}
