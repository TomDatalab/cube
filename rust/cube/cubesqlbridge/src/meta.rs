//! The SQL API's [`MetaContext`], built from a [`cubemodel::DataModel`].
//!
//! The Node.js bridge asks `ApiGateway.meta` for the cube list and
//! `ApiGateway.sqlGenerators` for `memberToDataSource` /
//! `dataSourceToSqlGenerator`. All three come from the compiled model here,
//! with no JavaScript in between.

use std::collections::HashMap;
use std::sync::Arc;

use cubemodel::meta::rest_meta_response_with;
use cubemodel::{meta_config, CubeDef, DataModel};
use cubesql::transport::{CubeMeta, MetaContext, SqlGenerator};
use cubesql::CubeError;
use uuid::Uuid;

use crate::sql_generator::RustSqlGenerator;

/// The data source a cube without an explicit `data_source:` belongs to, the
/// same fallback `CompilerApi.memberToDataSource` uses.
pub const DEFAULT_DATA_SOURCE: &str = "default";

/// The namespace the compiler id is derived in. Any fixed UUID does; this one
/// is arbitrary but must never change, or every deployment's compiler id
/// changes with it and every cached compilation is invalidated once.
const COMPILER_ID_NAMESPACE: Uuid = Uuid::from_bytes([
    0x1f, 0x4c, 0x2b, 0x7a, 0x9d, 0x3e, 0x4f, 0x81, 0xb2, 0x60, 0x0a, 0x5c, 0x8e, 0x11, 0x37, 0xd4,
]);

/// `ApiGateway.meta` for the SQL API: the cube list in the REST `/v1/meta`
/// shape, parsed into the types cubesql indexes.
///
/// `include_hidden` is the dev-mode branch of `filterVisibleItemsInMeta`;
/// production hides non-public members from the SQL API too.
pub fn cube_meta(model: &DataModel, include_hidden: bool) -> Result<Vec<CubeMeta>, CubeError> {
    let config = meta_config(model);
    let response = rest_meta_response_with(&config, false, include_hidden);
    let cubes = response
        .get("cubes")
        .cloned()
        .unwrap_or_else(|| serde_json::Value::Array(Vec::new()));

    serde_json::from_value(cubes)
        .map_err(|e| CubeError::internal(format!("Failed to read the compiled meta config: {e}")))
}

/// `CompilerApi.memberToDataSource`: `"<cube>.<member>" -> data source`.
///
/// A view's member resolves to the data source of the cube it was included
/// from, which is how one view can span several data sources.
pub fn member_to_data_source(model: &DataModel) -> HashMap<String, String> {
    let mut mapping = HashMap::new();

    for cube in model.cube_list() {
        if cube.is_view {
            for included in &cube.included_members {
                let source_cube = included
                    .member_path
                    .split('.')
                    .next()
                    .unwrap_or(&included.member_path);
                let data_source = model
                    .get(source_cube)
                    .map(data_source_of)
                    .unwrap_or_else(|| DEFAULT_DATA_SOURCE.to_string());
                mapping.insert(format!("{}.{}", cube.name, included.name), data_source);
            }
        } else {
            let data_source = data_source_of(cube);
            for member in cube
                .dimensions
                .keys()
                .chain(cube.measures.keys())
                .chain(cube.segments.keys())
            {
                mapping.insert(format!("{}.{}", cube.name, member), data_source.clone());
            }
        }
    }

    mapping
}

fn data_source_of(cube: &CubeDef) -> String {
    cube.data_source
        .clone()
        .unwrap_or_else(|| DEFAULT_DATA_SOURCE.to_string())
}

/// A stable id for one compiled model.
///
/// Node derives it from the compiled schema and hands it to the SQL API, which
/// keys its compiler cache on it. The requirement is only that it is stable for
/// one model and different for another, so it is a v5 UUID over the model's own
/// serialization: two processes reading the same files agree on it, and any
/// change to a cube - including one that never reaches `/v1/meta`, like a
/// member's `sql:` - produces a different id.
pub fn compiler_id(model: &DataModel) -> Uuid {
    let mut canonical = Vec::new();

    for cube in model.cube_list() {
        canonical.extend_from_slice(cube.name.as_bytes());
        canonical.push(0);
        match serde_json::to_vec(cube) {
            Ok(serialized) => canonical.extend_from_slice(&serialized),
            // A cube that cannot be serialized still has to contribute
            // something, or two different models could share an id.
            Err(e) => canonical.extend_from_slice(format!("<unserializable: {e}>").as_bytes()),
        }
        canonical.push(0);
    }

    Uuid::new_v5(&COMPILER_ID_NAMESPACE, &canonical)
}

/// One [`SqlGenerator`] per data source the model uses.
///
/// `dialects` overrides the dialect of a named data source; anything not named
/// there is rendered through `default_dialect`.
pub fn data_source_to_sql_generator(
    member_to_data_source: &HashMap<String, String>,
    default_dialect: cubeplanner::Dialect,
    dialects: &HashMap<String, cubeplanner::Dialect>,
) -> Result<HashMap<String, Arc<dyn SqlGenerator + Send + Sync>>, CubeError> {
    let mut data_sources: Vec<&str> = member_to_data_source
        .values()
        .map(String::as_str)
        .chain(std::iter::once(DEFAULT_DATA_SOURCE))
        .chain(dialects.keys().map(String::as_str))
        .collect();
    data_sources.sort_unstable();
    data_sources.dedup();

    let mut generators: HashMap<String, Arc<dyn SqlGenerator + Send + Sync>> = HashMap::new();
    for data_source in data_sources {
        let dialect = dialects
            .get(data_source)
            .copied()
            .unwrap_or(default_dialect);
        generators.insert(
            data_source.to_string(),
            Arc::new(RustSqlGenerator::new(dialect)?),
        );
    }

    Ok(generators)
}

/// Everything the SQL API needs to know about one compiled model.
pub fn meta_context(
    model: &DataModel,
    include_hidden: bool,
    default_dialect: cubeplanner::Dialect,
    dialects: &HashMap<String, cubeplanner::Dialect>,
) -> Result<Arc<MetaContext>, CubeError> {
    let cubes = cube_meta(model, include_hidden)?;
    let member_to_data_source = member_to_data_source(model);
    let generators =
        data_source_to_sql_generator(&member_to_data_source, default_dialect, dialects)?;

    Ok(Arc::new(MetaContext::new(
        cubes,
        member_to_data_source,
        generators,
        compiler_id(model),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cubeplanner::Dialect;

    const MODEL: &str = include_str!("../tests/model.yml");

    fn load(source: &str) -> DataModel {
        cubemodel::ModelLoader::load_str(source, "model.yml").expect("the model should compile")
    }

    fn context(model: &DataModel) -> Arc<MetaContext> {
        meta_context(model, false, Dialect::Postgres, &HashMap::new())
            .expect("the meta context should build")
    }

    #[test]
    fn meta_context_carries_every_cube_and_view() {
        let meta = context(&load(MODEL));

        let mut names: Vec<&str> = meta.cubes.iter().map(|cube| cube.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, vec!["orders", "sales", "users"]);

        let orders = meta
            .find_cube_with_name("orders")
            .expect("orders should be in the meta context");
        assert!(orders.measures.iter().any(|m| m.name == "orders.count"));
        assert!(orders.dimensions.iter().any(|d| d.name == "orders.status"));
        assert!(orders.segments.iter().any(|s| s.name == "orders.completed"));
    }

    #[test]
    fn meta_context_builds_the_tables_cubesql_lists() {
        let meta = context(&load(MODEL));

        let orders = meta
            .tables
            .iter()
            .find(|table| table.name == "orders")
            .expect("orders should be a table");
        assert!(orders.columns.iter().any(|column| column.name == "status"));
        // Every table gets its own oid range, which is what the pg catalog
        // views are built from.
        assert!(meta.tables.iter().all(|table| table.oid >= 18000));
    }

    #[test]
    fn member_to_data_source_follows_the_cube_and_its_views() {
        let model = load(MODEL);
        let mapping = member_to_data_source(&model);

        assert_eq!(
            mapping.get("orders.count").map(String::as_str),
            Some(DEFAULT_DATA_SOURCE)
        );
        assert_eq!(
            mapping.get("orders.status").map(String::as_str),
            Some(DEFAULT_DATA_SOURCE)
        );
        assert_eq!(
            mapping.get("users.city").map(String::as_str),
            Some("warehouse")
        );

        // A view member keeps the data source of the cube it came from, so one
        // view can span two of them.
        let view_sources: Vec<&str> = mapping
            .iter()
            .filter(|(member, _)| member.starts_with("sales."))
            .map(|(_, source)| source.as_str())
            .collect();
        assert!(view_sources.contains(&DEFAULT_DATA_SOURCE));
        assert!(view_sources.contains(&"warehouse"));
    }

    #[test]
    fn every_data_source_gets_a_sql_generator() {
        let meta = context(&load(MODEL));

        let mut sources: Vec<&str> = meta
            .data_source_to_sql_generator
            .keys()
            .map(String::as_str)
            .collect();
        sources.sort_unstable();
        assert_eq!(sources, vec!["default", "warehouse"]);

        let generator = &meta.data_source_to_sql_generator["default"];
        assert!(generator
            .get_sql_templates()
            .contains_template("statements/select"));
    }

    #[test]
    fn a_data_source_can_render_in_its_own_dialect() {
        let model = load(MODEL);
        let dialects = HashMap::from([("warehouse".to_string(), Dialect::MySql)]);
        let meta = meta_context(&model, false, Dialect::Postgres, &dialects)
            .expect("the meta context should build");

        // Postgres numbers its parameters, MySQL uses positional `?`.
        assert!(
            meta.data_source_to_sql_generator["default"]
                .get_sql_templates()
                .reuse_params
        );
        assert!(
            !meta.data_source_to_sql_generator["warehouse"]
                .get_sql_templates()
                .reuse_params
        );
    }

    #[test]
    fn compiler_id_is_stable_for_the_same_model() {
        assert_eq!(compiler_id(&load(MODEL)), compiler_id(&load(MODEL)));
    }

    #[test]
    fn compiler_id_changes_when_the_model_changes() {
        let original = compiler_id(&load(MODEL));

        // A member renamed: visible in `/v1/meta`.
        let renamed = MODEL.replace("name: user_id", "name: buyer_id");
        assert_ne!(compiler_id(&load(&renamed)), original);

        // A member's SQL changed, which never reaches `/v1/meta` but does
        // change every query the model produces.
        let resqled = MODEL.replace("sql_table: public.orders", "sql_table: public.orders_v2");
        assert_ne!(compiler_id(&load(&resqled)), original);

        // A whole cube removed.
        let trimmed = MODEL
            .split("views:")
            .next()
            .expect("the model has a views section");
        assert_ne!(compiler_id(&load(trimmed)), original);
    }

    #[test]
    fn hidden_members_are_filtered_unless_asked_for() {
        let hidden = MODEL.replace(
            "      - name: total_amount\n        type: sum\n        sql: amount\n",
            "      - name: total_amount\n        type: sum\n        sql: amount\n        public: false\n",
        );
        assert_ne!(hidden, MODEL, "the fixture should have been rewritten");
        let model = load(&hidden);

        let visible = cube_meta(&model, false).expect("meta should build");
        let orders = visible
            .iter()
            .find(|cube| cube.name == "orders")
            .expect("orders");
        assert!(!orders
            .measures
            .iter()
            .any(|m| m.name == "orders.total_amount"));

        let all = cube_meta(&model, true).expect("meta should build");
        let orders = all
            .iter()
            .find(|cube| cube.name == "orders")
            .expect("orders");
        assert!(orders
            .measures
            .iter()
            .any(|m| m.name == "orders.total_amount"));
    }
}
