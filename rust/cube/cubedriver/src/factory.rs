//! `DriverFactory`: instantiates a driver from a `CUBEJS_DB_TYPE` value.

use std::sync::Arc;

use crate::bigquery::{BigQueryConfig, BigQueryDriver};
use crate::clickhouse::{ClickHouseConfig, ClickHouseDriver};
use crate::config::DriverConfig;
use crate::cubestore::{CubeStoreConfig, CubeStoreDriver};
use crate::driver::Driver;
use crate::druid::{DruidConfig, DruidDriver};
use crate::error::{DriverError, Result};
use crate::firebolt::{FireboltConfig, FireboltDriver};
use crate::mssql::{MsSqlConfig, MsSqlDriver};
use crate::mysql::{MySqlConfig, MySqlDriver};
use crate::postgres::{PostgresConfig, PostgresDriver};
use crate::redshift::{RedshiftConfig, RedshiftDriver};
use crate::snowflake::{SnowflakeConfig, SnowflakeDriver};

/// Every `CUBEJS_DB_TYPE` known to the Node.js server
/// (`packages/cubejs-server-core/src/core/DriverDependencies.ts`).
pub const KNOWN_DB_TYPES: &[&str] = &[
    "postgres",
    "mysql",
    "mysqlauroraserverless",
    "mssql",
    "athena",
    "jdbc",
    "mongobi",
    "bigquery",
    "redshift",
    "clickhouse",
    "crate",
    "firebolt",
    "hive",
    "snowflake",
    "prestodb",
    "trino",
    "oracle",
    "sqlite",
    "dremio",
    "druid",
    "duckdb",
    "cubestore",
    "ksql",
    "questdb",
    "materialize",
    "vertica",
    "pinot",
    "databricks-jdbc",
];

/// Database types with a Rust implementation.
pub const IMPLEMENTED_DB_TYPES: &[&str] = &[
    "postgres",
    "mysql",
    "clickhouse",
    "cubestore",
    "bigquery",
    "snowflake",
    "mssql",
    "redshift",
    "druid",
    "firebolt",
    #[cfg(feature = "cratedb")]
    "crate",
    #[cfg(feature = "materialize")]
    "materialize",
    #[cfg(feature = "questdb")]
    "questdb",
    #[cfg(feature = "vertica")]
    "vertica",
    #[cfg(feature = "mongobi")]
    "mongobi",
    #[cfg(feature = "prestodb")]
    "prestodb",
    #[cfg(feature = "trino")]
    "trino",
    #[cfg(feature = "pinot")]
    "pinot",
    #[cfg(feature = "athena")]
    "athena",
    #[cfg(feature = "mysqlauroraserverless")]
    "mysqlauroraserverless",
    #[cfg(feature = "dremio")]
    "dremio",
    #[cfg(feature = "ksql")]
    "ksql",
    #[cfg(feature = "databricks")]
    "databricks-jdbc",
    #[cfg(feature = "hive")]
    "hive",
    #[cfg(feature = "oracle")]
    "oracle",
    #[cfg(feature = "sqlite")]
    "sqlite",
    #[cfg(feature = "duckdb")]
    "duckdb",
];

/// Why the generic `jdbc` type is gone: it loads JDBC jars into a JVM, which
/// the pure-Rust server does not embed. Every database its presets covered
/// (`supported-drivers.ts`) has a native driver.
pub const JDBC_DROPPED_REASON: &str = "the generic JDBC driver needs a Java runtime. \
Use the native driver instead: CUBEJS_DB_TYPE=mysql, athena or hive \
(hive also serves Spark SQL through the Spark Thrift server), or \
databricks-jdbc for Databricks";

/// Creates drivers by database type.
#[derive(Debug, Default, Clone, Copy)]
pub struct DriverFactory;

impl DriverFactory {
    /// Whether `db_type` is a valid Cube driver name (implemented or not).
    pub fn is_known(db_type: &str) -> bool {
        KNOWN_DB_TYPES.contains(&db_type)
    }

    /// Whether `db_type` has a Rust implementation.
    pub fn is_implemented(db_type: &str) -> bool {
        IMPLEMENTED_DB_TYPES.contains(&db_type)
    }

    /// Instantiates the driver for `db_type` with `config`.
    ///
    /// Returns [`DriverError::UnsupportedDriver`] for known-but-unported
    /// types and [`DriverError::UnknownDriver`] for anything else.
    pub fn create(db_type: &str, config: DriverConfig) -> Result<Arc<dyn Driver>> {
        match db_type {
            "postgres" => {
                let driver = PostgresDriver::new(PostgresConfig::from_driver_config(config))?;
                Ok(Arc::new(driver))
            }
            "mysql" => {
                let driver = MySqlDriver::new(MySqlConfig::from_driver_config(config))?;
                Ok(Arc::new(driver))
            }
            "clickhouse" => {
                let driver = ClickHouseDriver::new(ClickHouseConfig::from_driver_config(config))?;
                Ok(Arc::new(driver))
            }
            "bigquery" => {
                let mut bigquery = BigQueryConfig::from_driver_config(config);
                bigquery.apply_env()?;
                Ok(Arc::new(BigQueryDriver::new(bigquery)?))
            }
            "snowflake" => {
                let mut snowflake = SnowflakeConfig::from_driver_config(config);
                snowflake.apply_env()?;
                Ok(Arc::new(SnowflakeDriver::new(snowflake)?))
            }
            "mssql" => {
                let mut mssql = MsSqlConfig::from_driver_config(config);
                mssql.apply_env();
                Ok(Arc::new(MsSqlDriver::new(mssql)?))
            }
            "redshift" => {
                let mut redshift = RedshiftConfig::from_driver_config(config);
                redshift.apply_env();
                Ok(Arc::new(RedshiftDriver::new(redshift)?))
            }
            "druid" => {
                let druid = DruidConfig::from_driver_config(config)?;
                Ok(Arc::new(DruidDriver::new(druid)?))
            }
            "firebolt" => {
                let mut firebolt = FireboltConfig::from_driver_config(config);
                firebolt.apply_env();
                Ok(Arc::new(FireboltDriver::new(firebolt)?))
            }
            // Cube Store is configured through CUBEJS_CUBESTORE_*, not
            // CUBEJS_DB_*; `config` only carries the shared driver knobs.
            "cubestore" => {
                let driver = CubeStoreDriver::new(CubeStoreConfig {
                    driver: config,
                    ..CubeStoreConfig::from_env()?
                })?;
                Ok(Arc::new(driver))
            }
            #[cfg(feature = "cratedb")]
            "crate" => Ok(Arc::new(crate::crate_db::CrateDriver::new(
                crate::crate_db::CrateConfig::from_driver_config(config),
            )?)),
            #[cfg(feature = "materialize")]
            "materialize" => {
                let mut materialize =
                    crate::materialize::MaterializeConfig::from_driver_config(config);
                materialize.apply_env();
                Ok(Arc::new(crate::materialize::MaterializeDriver::new(
                    materialize,
                )?))
            }
            #[cfg(feature = "questdb")]
            "questdb" => Ok(Arc::new(crate::questdb::QuestDriver::new(
                crate::questdb::QuestConfig::from_driver_config(config),
            )?)),
            #[cfg(feature = "vertica")]
            "vertica" => Ok(Arc::new(crate::vertica::VerticaDriver::new(
                crate::vertica::VerticaConfig::from_driver_config(config),
            )?)),
            #[cfg(feature = "mongobi")]
            "mongobi" => Ok(Arc::new(crate::mongobi::MongoBiDriver::new(
                crate::mongobi::MongoBiConfig::from_driver_config(config),
            )?)),
            #[cfg(feature = "prestodb")]
            "prestodb" => {
                let mut presto = crate::prestodb::PrestoConfig::from_driver_config(
                    config,
                    crate::prestodb::Engine::Presto,
                );
                presto.apply_env()?;
                Ok(Arc::new(crate::prestodb::PrestoDriver::new(presto)?))
            }
            #[cfg(feature = "trino")]
            "trino" => {
                let mut trino = crate::trino::trino_config_from_driver_config(config);
                trino.apply_env()?;
                Ok(Arc::new(crate::trino::TrinoDriver::new(trino)?))
            }
            #[cfg(feature = "pinot")]
            "pinot" => {
                let mut pinot = crate::pinot::PinotConfig::from_driver_config(config);
                pinot.apply_env()?;
                Ok(Arc::new(crate::pinot::PinotDriver::new(pinot)?))
            }
            #[cfg(feature = "athena")]
            "athena" => {
                let mut athena = crate::athena::AthenaConfig::from_driver_config(config);
                athena.apply_env()?;
                Ok(Arc::new(crate::athena::AthenaDriver::new(athena)?))
            }
            #[cfg(feature = "mysqlauroraserverless")]
            "mysqlauroraserverless" => {
                use crate::mysql_aurora_serverless::{
                    AuroraServerlessMySqlConfig, AuroraServerlessMySqlDriver,
                };
                let mut aurora = AuroraServerlessMySqlConfig::from_driver_config(config);
                aurora.apply_env()?;
                Ok(Arc::new(AuroraServerlessMySqlDriver::new(aurora)?))
            }
            #[cfg(feature = "dremio")]
            "dremio" => {
                let mut dremio = crate::dremio::DremioConfig::from_driver_config(config);
                dremio.apply_env()?;
                Ok(Arc::new(crate::dremio::DremioDriver::new(dremio)?))
            }
            #[cfg(feature = "ksql")]
            "ksql" => {
                let mut ksql = crate::ksql::KsqlConfig::from_driver_config(config);
                ksql.apply_env()?;
                Ok(Arc::new(crate::ksql::KsqlDriver::new(ksql)?))
            }
            #[cfg(feature = "databricks")]
            "databricks-jdbc" => {
                let mut databricks =
                    crate::databricks::DatabricksConfig::from_driver_config(config);
                databricks.apply_env()?;
                Ok(Arc::new(crate::databricks::DatabricksDriver::new(
                    databricks,
                )?))
            }
            #[cfg(feature = "hive")]
            "hive" => {
                let mut hive = crate::hive::HiveConfig::from_driver_config(config);
                hive.apply_env()?;
                Ok(Arc::new(crate::hive::HiveDriver::new(hive)?))
            }
            #[cfg(feature = "oracle")]
            "oracle" => Ok(Arc::new(crate::oracle::OracleDriver::new(
                crate::oracle::OracleConfig::from_driver_config(config),
            )?)),
            #[cfg(feature = "sqlite")]
            "sqlite" => Ok(Arc::new(crate::sqlite::SqliteDriver::new(
                crate::sqlite::SqliteConfig::from_driver_config(config),
            )?)),
            #[cfg(feature = "duckdb")]
            "duckdb" => {
                let mut duckdb = crate::duckdb::DuckDbConfig::from_driver_config(config);
                duckdb.apply_env()?;
                Ok(Arc::new(crate::duckdb::DuckDbDriver::new(duckdb)?))
            }
            "jdbc" => Err(DriverError::DroppedDriver {
                db_type: "jdbc".to_string(),
                reason: JDBC_DROPPED_REASON.to_string(),
            }),
            other if Self::is_known(other) => {
                Err(DriverError::UnsupportedDriver(other.to_string()))
            }
            other => Err(DriverError::UnknownDriver(other.to_string())),
        }
    }

    /// Reads `CUBEJS_DB_TYPE` (and the rest of the configuration) for
    /// `data_source` from the environment and instantiates the driver.
    pub fn create_from_env(data_source: Option<&str>) -> Result<Arc<dyn Driver>> {
        let config = DriverConfig::from_env(data_source)?;
        let db_type = config.data_source.db_type.clone().ok_or_else(|| {
            DriverError::Config(format!(
                "CUBEJS_DB_TYPE is not set for the {} data source",
                config.data_source.data_source
            ))
        })?;
        Self::create(&db_type, config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unported_types_are_reported_clearly() {
        for t in KNOWN_DB_TYPES
            .iter()
            .filter(|t| !IMPLEMENTED_DB_TYPES.contains(t) && **t != "jdbc")
        {
            let err = DriverFactory::create(t, DriverConfig::default())
                .err()
                .expect("must fail");
            assert_eq!(
                err.to_string(),
                format!("Database driver is not implemented in Rust yet: {t}")
            );
        }
    }

    #[test]
    fn jdbc_is_dropped_with_a_pointer_to_native_drivers() {
        let err = DriverFactory::create("jdbc", DriverConfig::default())
            .err()
            .expect("must fail");
        let message = err.to_string();
        assert!(message.starts_with(
            "Database driver jdbc is not available in the Rust server: the generic JDBC driver needs a Java runtime."
        ));
        assert!(message.contains("CUBEJS_DB_TYPE=mysql, athena or hive"));
    }

    #[test]
    fn unknown_type() {
        let err = DriverFactory::create("nosuchdb", DriverConfig::default())
            .err()
            .expect("must fail");
        assert_eq!(err.to_string(), "Unknown database type: nosuchdb");
    }

    /// Minimal configuration a driver needs to be *constructed*.
    fn config_for(db_type: &str) -> DriverConfig {
        let mut config = DriverConfig::default();
        // `DruidDriver` refuses to start without an endpoint, like the Node
        // driver ("Please specify CUBEJS_DB_URL").
        if db_type == "druid" {
            config.data_source.url = Some("http://localhost:8888".to_string());
        }
        // Oracle builds its connect string from host and service name.
        if db_type == "oracle" {
            config.data_source.host = Some("localhost".to_string());
            config.data_source.database = Some("FREEPDB1".to_string());
        }
        // SQLite needs a file name (`CUBEJS_DB_NAME`); `:memory:` opens none.
        if db_type == "sqlite" {
            config.data_source.database = Some(":memory:".to_string());
        }
        // Pinot has no default broker port.
        if db_type == "pinot" {
            config.data_source.port = Some(8099);
        }
        config
    }

    /// Types whose constructor reads mandatory variables from the
    /// environment, as the Node.js drivers do; created without them, they
    /// fail with a configuration error naming the variable.
    const NEEDS_ENV: &[(&str, &str)] = &[
        ("databricks-jdbc", "CUBEJS_DB_DATABRICKS_URL"),
        ("mysqlauroraserverless", "secretArn"),
    ];

    #[test]
    fn types_with_mandatory_variables_name_them() {
        for (db_type, variable) in NEEDS_ENV {
            if !DriverFactory::is_implemented(db_type) {
                continue;
            }
            // Only meaningful when the variable is not set in this process.
            if std::env::var(variable).is_ok() {
                continue;
            }
            let err = DriverFactory::create(db_type, config_for(db_type))
                .err()
                .unwrap_or_else(|| panic!("{db_type} must fail without {variable}"));
            assert!(err.to_string().contains(variable), "{db_type}: {err}");
        }
    }

    #[tokio::test]
    async fn every_implemented_type_is_created_without_connecting() {
        for db_type in IMPLEMENTED_DB_TYPES {
            if NEEDS_ENV.iter().any(|(t, _)| t == db_type) {
                continue;
            }
            let driver = DriverFactory::create(db_type, config_for(db_type))
                .unwrap_or_else(|e| panic!("{db_type}: {e}"));
            driver.release().await.unwrap();
        }
    }

    #[test]
    fn druid_requires_an_endpoint() {
        let err = DriverFactory::create("druid", DriverConfig::default())
            .err()
            .expect("must fail");
        assert_eq!(err.to_string(), "Please specify CUBEJS_DB_URL");
    }

    #[tokio::test]
    async fn the_new_drivers_keep_their_dialects() {
        let mssql = DriverFactory::create("mssql", DriverConfig::default()).unwrap();
        assert_eq!(mssql.param(0), "@_1");
        assert_eq!(
            mssql.wrap_query_with_limit("SELECT 1", 5),
            "SELECT TOP 5 * FROM (SELECT 1) AS t"
        );

        let redshift = DriverFactory::create("redshift", DriverConfig::default()).unwrap();
        assert_eq!(redshift.param(0), "$1");
        assert!(!redshift.read_only());
        assert!(redshift.primary_keys_query(None).is_none());

        let bigquery = DriverFactory::create("bigquery", DriverConfig::default()).unwrap();
        assert_eq!(bigquery.quote_identifier("Orders"), "`Orders`");

        let snowflake = DriverFactory::create("snowflake", DriverConfig::default()).unwrap();
        assert!(snowflake.capabilities().unload_without_temp_table);

        let firebolt = DriverFactory::create("firebolt", DriverConfig::default()).unwrap();
        assert!(firebolt.read_only());

        let druid = DriverFactory::create("druid", config_for("druid")).unwrap();
        assert!(druid.read_only());

        for d in [mssql, redshift, bigquery, snowflake, firebolt, druid] {
            d.release().await.unwrap();
        }
    }

    #[tokio::test]
    async fn driver_dialects_differ_per_type() {
        let mysql = DriverFactory::create("mysql", DriverConfig::default()).unwrap();
        assert_eq!(mysql.quote_identifier("a"), "`a`");
        assert_eq!(mysql.param(0), "?");
        assert!(mysql.capabilities().incremental_schema_loading);

        let clickhouse = DriverFactory::create("clickhouse", DriverConfig::default()).unwrap();
        assert_eq!(clickhouse.quote_identifier("a"), "\"a\"");
        assert!(clickhouse.capabilities().unload_without_temp_table);

        let cubestore = DriverFactory::create("cubestore", DriverConfig::default()).unwrap();
        assert_eq!(cubestore.quote_identifier("a"), "`a`");
        assert!(cubestore.capabilities().csv_import);
        assert!(cubestore.capabilities().stream_import);
        assert!(!cubestore.read_only());

        for d in [mysql, clickhouse, cubestore] {
            d.release().await.unwrap();
        }
    }

    #[tokio::test]
    async fn postgres_is_created_without_connecting() {
        let driver = DriverFactory::create("postgres", DriverConfig::default()).unwrap();
        assert_eq!(driver.param(0), "$1");
        assert!(driver.capabilities().incremental_schema_loading);
        driver.release().await.unwrap();
    }
}
