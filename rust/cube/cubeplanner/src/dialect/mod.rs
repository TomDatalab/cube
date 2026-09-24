//! SQL dialects: the `DriverTools` implementation and the Jinja template set
//! the planner renders through.
//!
//! Each dialect is one `DriverTools` implementation — the non-template
//! behaviour (`convertTz`, `timeGroupedColumn`, interval arithmetic, HLL) —
//! plus a `sqlTemplates()` delta over the base template set, exactly the way
//! the `BaseQuery` subclasses in `packages/cubejs-schema-compiler/src/adapter`
//! are layered.

mod driver;
mod interval;
mod templates;

use crate::error::PlannerError;
use cubesqlplanner::rust_model::{MockDriverTools, MockSqlTemplatesRender};
use std::rc::Rc;
use std::str::FromStr;

pub use cubesqlplanner::cube_bridge::driver_tools::DriverTools;
pub use driver::SqlDialectTools;
pub use interval::{split_sql_interval, ParsedInterval};

/// The flattened `"<group>/<name>" -> Jinja template` set a dialect renders
/// through.
pub type SqlTemplates = MockSqlTemplatesRender;

/// The base template set every dialect starts from — the JS
/// `BaseQuery.sqlTemplates()`.
pub fn base_templates() -> SqlTemplates {
    SqlTemplates::try_new(templates::base_map())
        .expect("Base templates should always parse successfully")
}

/// Postgres: the base templates plus the `PostgresQuery` deltas, `$n`
/// parameters and `date_trunc`-based time grouping.
pub struct PostgresDialect;

impl PostgresDialect {
    pub fn templates() -> SqlTemplates {
        templates::postgres().expect("Postgres templates should always parse successfully")
    }

    pub fn driver_tools(timezone: &str) -> Rc<dyn DriverTools> {
        Rc::new(MockDriverTools::with_sql_templates_and_timezone(
            Self::templates(),
            timezone.to_string(),
        ))
    }
}

/// CubeStore: the dialect external pre-aggregations are read through.
/// Renders positional `?` parameters, so a value is never shared between two
/// placeholders.
pub struct CubeStoreDialect;

impl CubeStoreDialect {
    pub fn templates() -> SqlTemplates {
        SqlTemplates::cubestore_templates()
    }

    pub fn driver_tools(timezone: &str) -> Rc<dyn DriverTools> {
        Rc::new(
            MockDriverTools::with_sql_templates_and_timezone(
                Self::templates(),
                timezone.to_string(),
            )
            .with_cubestore_dialect()
            .without_params_reuse(),
        )
    }
}

/// Which dialect the SQL is rendered for.
///
/// One variant per `BaseQuery` subclass whose SQL differs. Data source types
/// that share a class share a variant: see [`Dialect::for_db_type`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Dialect {
    #[default]
    Postgres,
    #[serde(alias = "cubestore")]
    CubeStore,
    #[serde(alias = "mysql")]
    MySql,
    #[serde(alias = "clickhouse")]
    ClickHouse,
    #[serde(alias = "bigquery")]
    BigQuery,
    Snowflake,
    Databricks,
    #[serde(alias = "mssql", alias = "sqlserver")]
    MsSql,
    Redshift,
    #[serde(alias = "mongobi")]
    MongoBi,
    /// PrestoDB, and Athena, whose `AthenaQuery` adds nothing to it.
    #[serde(alias = "prestodb", alias = "athena")]
    Presto,
    Trino,
    Vertica,
    #[serde(alias = "cratedb")]
    Crate,
    Hive,
    Oracle,
    Sqlite,
    Druid,
    Firebolt,
    Dremio,
    Ksql,
    #[serde(alias = "questdb")]
    QuestDb,
    Pinot,
    #[serde(alias = "duckdb")]
    DuckDb,
}

/// The name of every `CUBEJS_DB_TYPE` the Rust planner renders SQL for, and
/// its dialect — `ADAPTERS` in `adapter/QueryBuilder.ts` plus the drivers that
/// bring their own `BaseQuery` subclass.
pub const DB_TYPE_DIALECTS: [(&str, Dialect); 29] = [
    ("postgres", Dialect::Postgres),
    ("redshift", Dialect::Redshift),
    ("mysql", Dialect::MySql),
    ("mysqlauroraserverless", Dialect::MySql),
    ("mongobi", Dialect::MongoBi),
    ("mssql", Dialect::MsSql),
    ("bigquery", Dialect::BigQuery),
    ("prestodb", Dialect::Presto),
    ("qubole_prestodb", Dialect::Presto),
    ("athena", Dialect::Presto),
    ("trino", Dialect::Trino),
    ("vertica", Dialect::Vertica),
    ("snowflake", Dialect::Snowflake),
    ("clickhouse", Dialect::ClickHouse),
    ("crate", Dialect::Crate),
    ("hive", Dialect::Hive),
    ("oracle", Dialect::Oracle),
    ("sqlite", Dialect::Sqlite),
    ("materialize", Dialect::Postgres),
    ("cubestore", Dialect::CubeStore),
    ("druid", Dialect::Druid),
    ("firebolt", Dialect::Firebolt),
    ("dremio", Dialect::Dremio),
    ("ksql", Dialect::Ksql),
    ("questdb", Dialect::QuestDb),
    ("pinot", Dialect::Pinot),
    ("duckdb", Dialect::DuckDb),
    ("databricks-jdbc", Dialect::Databricks),
    ("databricks", Dialect::Databricks),
];

impl Dialect {
    /// Every dialect the Rust planner renders, in the order they are listed in
    /// an error message.
    pub const ALL: [Dialect; 24] = [
        Dialect::Postgres,
        Dialect::CubeStore,
        Dialect::MySql,
        Dialect::ClickHouse,
        Dialect::BigQuery,
        Dialect::Snowflake,
        Dialect::Databricks,
        Dialect::MsSql,
        Dialect::Redshift,
        Dialect::MongoBi,
        Dialect::Presto,
        Dialect::Trino,
        Dialect::Vertica,
        Dialect::Crate,
        Dialect::Hive,
        Dialect::Oracle,
        Dialect::Sqlite,
        Dialect::Druid,
        Dialect::Firebolt,
        Dialect::Dremio,
        Dialect::Ksql,
        Dialect::QuestDb,
        Dialect::Pinot,
        Dialect::DuckDb,
    ];

    /// The dialect a data source of type `db_type` (`CUBEJS_DB_TYPE`, or a
    /// data source's `type` in `cube.yml`) is planned in.
    ///
    /// Unlike [`FromStr`], which reads a dialect's own name, this reads a
    /// driver type, and an unknown one is an error rather than a guess: a
    /// data source planned in the wrong dialect answers with SQL its database
    /// rejects, or worse, reads differently.
    pub fn for_db_type(db_type: &str) -> Result<Dialect, PlannerError> {
        let normalized = db_type.trim().to_ascii_lowercase();
        DB_TYPE_DIALECTS
            .iter()
            .find(|(name, _)| *name == normalized)
            .map(|(_, dialect)| *dialect)
            .ok_or_else(|| {
                let known: Vec<&str> = DB_TYPE_DIALECTS.iter().map(|(name, _)| *name).collect();
                PlannerError::unsupported(format!(
                    "The Rust SQL planner has no dialect for data source type `{db_type}`. \
                     Supported types: {}",
                    known.join(", ")
                ))
            })
    }

    /// The canonical name, as [`FromStr`] and serde spell it.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::CubeStore => "cube_store",
            Self::MySql => "my_sql",
            Self::ClickHouse => "click_house",
            Self::BigQuery => "big_query",
            Self::Snowflake => "snowflake",
            Self::Databricks => "databricks",
            Self::MsSql => "ms_sql",
            Self::Redshift => "redshift",
            Self::MongoBi => "mongo_bi",
            Self::Presto => "presto",
            Self::Trino => "trino",
            Self::Vertica => "vertica",
            Self::Crate => "crate",
            Self::Hive => "hive",
            Self::Oracle => "oracle",
            Self::Sqlite => "sqlite",
            Self::Druid => "druid",
            Self::Firebolt => "firebolt",
            Self::Dremio => "dremio",
            Self::Ksql => "ksql",
            Self::QuestDb => "quest_db",
            Self::Pinot => "pinot",
            Self::DuckDb => "duck_db",
        }
    }

    pub fn templates(&self) -> SqlTemplates {
        self.try_templates()
            .expect("Dialect templates should always parse successfully")
    }

    fn try_templates(&self) -> Result<SqlTemplates, PlannerError> {
        match self {
            Self::Postgres => templates::postgres(),
            Self::CubeStore => templates::cubestore(),
            Self::MySql => templates::mysql(),
            Self::ClickHouse => templates::clickhouse(),
            Self::BigQuery => templates::bigquery(),
            Self::Snowflake => templates::snowflake(),
            Self::Databricks => templates::databricks(),
            Self::MsSql => templates::mssql(),
            Self::Redshift => templates::redshift(),
            Self::MongoBi => templates::mongobi(),
            Self::Presto | Self::Trino => templates::presto(),
            Self::Vertica => templates::vertica(),
            Self::Crate => templates::crate_db(),
            Self::Hive => templates::hive(),
            Self::Oracle => templates::oracle(),
            Self::Sqlite => templates::sqlite(),
            Self::Druid => templates::druid(),
            Self::Firebolt => templates::firebolt(),
            Self::Dremio => templates::dremio(),
            Self::Ksql => templates::ksql(),
            Self::QuestDb => templates::questdb(),
            Self::Pinot => templates::pinot(),
            Self::DuckDb => templates::duckdb(),
        }
    }

    pub fn driver_tools(&self, timezone: &str) -> Rc<dyn DriverTools> {
        match self {
            // Postgres and CubeStore keep the implementation the planner's own
            // fixtures carry, so both crates render them identically.
            Self::Postgres => PostgresDialect::driver_tools(timezone),
            Self::CubeStore => CubeStoreDialect::driver_tools(timezone),
            other => Rc::new(SqlDialectTools::new(*other, timezone, other.templates())),
        }
    }

    /// Rejects rendered SQL that uses a construct this dialect's database does
    /// not have, where the planner has no dialect-specific way around it.
    ///
    /// The JS dialects rewrite these queries (`QuestQuery.baseHaving` wraps
    /// the query instead of emitting `HAVING`); the Rust planner renders them
    /// through shared code, so the query is refused with a named error rather
    /// than sent to a database that will reject it.
    pub fn check_rendered_sql(&self, sql: &str) -> Result<(), PlannerError> {
        let unsupported: &[(&str, &str)] = match self {
            // `QuestQuery.baseHaving`: QuestDB has no HAVING clause.
            Self::QuestDb => &[(
                "\nHAVING ",
                "a filter on a measure (QuestDB has no HAVING clause)",
            )],
            // The BI Connector documents no OVER clause.
            Self::MongoBi => &[(
                " OVER (",
                "window functions (rolling, ranked and multi-stage measures)",
            )],
            _ => &[],
        };
        match unsupported.iter().find(|(needle, _)| sql.contains(needle)) {
            Some((_, feature)) => Err(PlannerError::unsupported(format!(
                "{}{} dialect: {feature}",
                crate::error::UNSUPPORTED_BY_DIALECT,
                self.as_str()
            ))),
            None => Ok(()),
        }
    }
}

impl FromStr for Dialect {
    type Err = crate::PlannerError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        match name.to_ascii_lowercase().replace(['-', '_'], "").as_str() {
            "postgres" | "postgresql" | "pg" => Ok(Self::Postgres),
            "cubestore" => Ok(Self::CubeStore),
            "mysql" => Ok(Self::MySql),
            "clickhouse" => Ok(Self::ClickHouse),
            "bigquery" => Ok(Self::BigQuery),
            "snowflake" => Ok(Self::Snowflake),
            "databricks" => Ok(Self::Databricks),
            "mssql" | "sqlserver" | "msssql" => Ok(Self::MsSql),
            "redshift" => Ok(Self::Redshift),
            "mongobi" => Ok(Self::MongoBi),
            "presto" | "prestodb" | "athena" => Ok(Self::Presto),
            "trino" => Ok(Self::Trino),
            "vertica" => Ok(Self::Vertica),
            "crate" | "cratedb" => Ok(Self::Crate),
            "hive" => Ok(Self::Hive),
            "oracle" => Ok(Self::Oracle),
            "sqlite" => Ok(Self::Sqlite),
            "druid" => Ok(Self::Druid),
            "firebolt" => Ok(Self::Firebolt),
            "dremio" => Ok(Self::Dremio),
            "ksql" => Ok(Self::Ksql),
            "questdb" => Ok(Self::QuestDb),
            "pinot" => Ok(Self::Pinot),
            "duckdb" => Ok(Self::DuckDb),
            other => Err(crate::PlannerError::unsupported(format!(
                "Unknown SQL dialect `{other}`. Supported dialects: {}",
                Dialect::ALL
                    .iter()
                    .map(Dialect::as_str)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }
}

impl std::fmt::Display for Dialect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_dialect_compiles_its_templates() {
        for dialect in Dialect::ALL {
            dialect
                .try_templates()
                .unwrap_or_else(|e| panic!("{dialect}: {e}"));
            dialect.driver_tools("UTC");
        }
        base_templates();
    }

    #[test]
    fn dialect_names_round_trip() {
        for dialect in Dialect::ALL {
            assert_eq!(Dialect::from_str(dialect.as_str()).unwrap(), dialect);
        }
        assert_eq!(Dialect::from_str("MySQL").unwrap(), Dialect::MySql);
        assert_eq!(Dialect::from_str("sqlserver").unwrap(), Dialect::MsSql);
        assert_eq!(Dialect::from_str("athena").unwrap(), Dialect::Presto);
        assert!(Dialect::from_str("elasticsearch").is_err());
    }
}
