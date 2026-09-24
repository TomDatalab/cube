//! Plans Cube queries from a YAML data model, in Rust, with no JavaScript.
//!
//! ```no_run
//! use cubeplanner::{plan, Model, PlanOptions, PlannerQuery};
//!
//! let model = Model::from_dir("model/cubes")?;
//! let query = PlannerQuery::from_json(r#"{"measures": ["orders.count"]}"#)?;
//! let planned = plan(&model, &query, &PlanOptions::postgres())?;
//! println!("{}", planned.sql);
//! # Ok::<(), cubeplanner::PlannerError>(())
//! ```
//!
//! # How it fits together
//!
//! The SQL planner itself is [`cubesqlplanner`] (Tesseract). It reaches the
//! data model through the `cube_bridge` traits, which in production are backed
//! by live JavaScript objects. This crate supplies pure-Rust implementations of
//! those traits instead:
//!
//! - [`Model`] parses the YAML data model.
//! - [`Evaluator`] implements `CubeEvaluator`, `BaseTools` and `JoinGraph` over
//!   a [`Model`], for one request.
//! - [`dialect`] implements `DriverTools` and carries the Jinja template set,
//!   for Postgres, CubeStore, MySQL, ClickHouse, BigQuery, Snowflake,
//!   Databricks and MS SQL.
//! - [`member_sql`] replaces the JS `MemberSqlTemplateCompiler`: it parses
//!   member `sql:` written in Cube's YAML syntax into the very same
//!   `CompiledMemberTemplate` the planner already consumes.
//! - [`member_expression`] replaces the gateway's `parseMemberExpression`: it
//!   reads the members the SQL API pushes down as expressions rather than
//!   names.
//!
//! [`plan`] answers with the SQL, its parameters in placeholder order, and the
//! map from result-set column name back to member that the REST response is
//! keyed by ([`PlannedSql::alias_name_to_member`]).
//!
//! # Where the implementation lives
//!
//! The model, the bridge implementations, the join graph, the dialects and the
//! member-sql parser live in `cubesqlplanner::rust_model` (behind that crate's
//! default-on `rust-model` feature) and are re-exported here. They started as
//! the planner's own test fixtures; keeping them in that crate means the
//! planner's ~1400 tests and this crate exercise exactly the same model code
//! rather than two copies that can drift. In particular the parser must live
//! beside the model, because the model's member definitions compile their
//! `sql:` lazily, while the planner walks them.
//!
//! # No scripting runtime
//!
//! Nothing here embeds JavaScript or Python. A model construct the parser
//! cannot express is rejected with a [`PlannerError`] naming the cube and
//! member, at [`Model`] load time, rather than silently planned as something
//! else. See [`Model::from_dir`] and [`member_sql`] for the exact list.
//!
//! The loader is strict for the same reason: an unknown key in a cube or a
//! member is a typo, not something to drop, and every `sql:` the model
//! carries — joins, `case` branches, measure `filters:` / `order_by:`,
//! granularity `sql:`, pre-aggregation reference lists — is parsed at load
//! time rather than when the planner first walks it.

pub mod dialect;
mod error;
mod evaluator;
pub mod member_expression;
mod model;
mod model_check;
mod options;
mod plan;
mod query;

pub use dialect::{
    base_templates, CubeStoreDialect, Dialect, DriverTools, PostgresDialect, SqlDialectTools,
    DB_TYPE_DIALECTS,
};
pub use error::PlannerError;
pub use evaluator::Evaluator;
pub use member_expression::{MemberExpression, QueryMember};
pub use model::Model;
pub use options::PlanOptions;
pub use plan::{plan, PlannedSql, QueryParam};
pub use query::{JoinHint, PlannerQuery, SubqueryJoin};

/// The member-`sql` parser: Cube's YAML member syntax to the planner's
/// `CompiledMemberTemplate`.
///
/// See [`ParsedMemberSql`](member_sql::ParsedMemberSql) for the supported
/// syntax.
pub mod member_sql {
    pub use cubesqlplanner::cube_bridge::member_sql::{
        CompiledFilterParamsColumn, CompiledMemberTemplate, FilterGroupItem, FilterParamsColumn,
        FilterParamsItem, SqlTemplate, SqlTemplateArgs,
    };
    pub use cubesqlplanner::rust_model::ParsedMemberSql;

    /// Parses one member `sql:` declaration.
    pub fn parse(sql: &str) -> Result<ParsedMemberSql, crate::PlannerError> {
        Ok(ParsedMemberSql::parse(sql)?)
    }

    /// Parses a reference list — a pre-aggregation's `dimensions:` /
    /// `measures:`, or a `rollup_references` declaration.
    pub fn parse_reference_list(
        members: &[String],
    ) -> Result<ParsedMemberSql, crate::PlannerError> {
        Ok(ParsedMemberSql::parse_reference_list(members)?)
    }
}

/// The join graph: Cube's `buildJoin` shortest-join-path search, in Rust.
pub mod join_graph {
    pub use cubesqlplanner::cube_bridge::join_definition::JoinDefinition;
    pub use cubesqlplanner::cube_bridge::join_hints::JoinHintItem;
    pub use cubesqlplanner::cube_bridge::join_item::JoinItem;
    pub use cubesqlplanner::rust_model::MockJoinGraph as JoinGraph;
}
