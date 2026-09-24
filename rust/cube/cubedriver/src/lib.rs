//! Rust port of Cube's database driver layer.
//!
//! * [`config`]: `CUBEJS_DB_*` environment parsing ([`DriverConfig`], [`DataSourceConfig`]).
//! * [`driver`]: the [`Driver`] trait (`DriverInterface` + `BaseDriver` defaults).
//! * [`types`]: row/column model and generic type mapping.
//! * [`bigquery`]: [`BigQueryDriver`] on the BigQuery REST API (`reqwest`).
//! * [`postgres`]: [`PostgresDriver`] on `tokio-postgres` + `deadpool-postgres` + `rustls`.
//! * [`cubestore`]: [`CubeStoreDriver`] on the `cubestore-ws-transport` WebSocket client.
//! * [`mysql`]: [`MySqlDriver`] on `mysql_async`.
//! * [`clickhouse`]: [`ClickHouseDriver`] on ClickHouse's HTTP interface (`reqwest`).
//! * [`mssql`]: [`MsSqlDriver`] on `tiberius` (TDS).
//! * [`firebolt`]: [`FireboltDriver`] on Firebolt's HTTP API.
//! * [`druid`]: [`DruidDriver`] on Druid's SQL HTTP endpoint.
//! * [`redshift`]: [`RedshiftDriver`], the PostgreSQL driver with Redshift's overrides.
//! * [`snowflake`]: [`SnowflakeDriver`] on the Snowflake SQL REST API.
//! * [`factory`]: [`DriverFactory`] mapping `CUBEJS_DB_TYPE` to an implementation.

pub mod bigquery;
pub mod clickhouse;
pub mod config;
pub mod csv_import;
#[cfg(feature = "databricks")]
pub mod databricks;
pub mod cubestore;
pub mod driver;
pub mod druid;
pub mod error;
pub mod escape;
pub mod factory;
pub mod firebolt;
#[cfg(feature = "hive")]
pub mod hive;
pub mod mssql;
pub mod mysql;
#[cfg(feature = "oracle")]
pub mod oracle;
pub mod postgres;
pub mod redshift;
pub mod snowflake;
pub mod sql;
pub mod type_detection;
pub mod types;
#[cfg(feature = "prestodb")]
pub mod prestodb;
#[cfg(feature = "prestodb")]
pub use prestodb::{PrestoConfig, PrestoDriver};
#[cfg(feature = "trino")]
pub mod trino;
#[cfg(feature = "trino")]
pub use trino::{TrinoConfig, TrinoDriver};
#[cfg(feature = "pinot")]
pub mod pinot;
#[cfg(feature = "pinot")]
pub use pinot::{PinotConfig, PinotDriver};
#[cfg(feature = "athena")]
pub mod athena;
#[cfg(feature = "mysqlauroraserverless")]
pub mod mysql_aurora_serverless;
#[cfg(all(test, any(feature = "athena", feature = "mysqlauroraserverless")))]
mod aws_test_server;
#[cfg(feature = "dremio")]
pub mod dremio;
#[cfg(feature = "ksql")]
pub mod ksql;
#[cfg(feature = "sqlite")]
pub mod sqlite;
#[cfg(feature = "duckdb")]
pub mod duckdb;

#[cfg(feature = "cratedb")]
pub mod crate_db;
#[cfg(feature = "materialize")]
pub mod materialize;
#[cfg(feature = "questdb")]
pub mod questdb;
#[cfg(feature = "vertica")]
pub mod vertica;
#[cfg(feature = "mongobi")]
pub mod mongobi;
#[cfg(feature = "cratedb")]
pub use crate_db::{CrateConfig, CrateDriver};
#[cfg(feature = "materialize")]
pub use materialize::{MaterializeConfig, MaterializeDriver};
#[cfg(feature = "questdb")]
pub use questdb::{QuestConfig, QuestDriver};
#[cfg(feature = "vertica")]
pub use vertica::{VerticaConfig, VerticaDriver};
#[cfg(feature = "mongobi")]
pub use mongobi::{MongoBiConfig, MongoBiDriver};

pub use bigquery::{BigQueryConfig, BigQueryDriver};
pub use clickhouse::{ClickHouseConfig, ClickHouseDriver};
pub use config::{DataSourceConfig, DriverConfig, EnvSource, ProcessEnv, SslConfig};
pub use csv_import::{csv_to_memory, CsvDialect};
pub use cubestore::{
    CreateTableOptions, CubeStoreCapability, CubeStoreConfig, CubeStoreDriver, SourceTable,
};
pub use driver::Driver;
#[cfg(feature = "databricks")]
pub use databricks::{DatabricksConfig, DatabricksDriver};
#[cfg(feature = "hive")]
pub use hive::{HiveConfig, HiveDriver};
#[cfg(feature = "athena")]
pub use athena::{AthenaConfig, AthenaDriver};
#[cfg(feature = "mysqlauroraserverless")]
pub use mysql_aurora_serverless::{AuroraServerlessMySqlConfig, AuroraServerlessMySqlDriver};
#[cfg(feature = "dremio")]
pub use dremio::{DremioConfig, DremioDriver};
#[cfg(feature = "ksql")]
pub use ksql::{KsqlConfig, KsqlDriver, KsqlStreamingTableData};
#[cfg(feature = "sqlite")]
pub use sqlite::{SqliteConfig, SqliteDriver};
#[cfg(feature = "duckdb")]
pub use crate::duckdb::{DuckDbConfig, DuckDbDriver};
pub use druid::{DruidConfig, DruidDriver};
pub use error::{DriverError, Result};
pub use factory::{DriverFactory, IMPLEMENTED_DB_TYPES, KNOWN_DB_TYPES};
pub use firebolt::{FireboltConfig, FireboltDriver};
pub use mssql::{MsSqlConfig, MsSqlDriver};
pub use mysql::{MySqlConfig, MySqlDriver};
#[cfg(feature = "oracle")]
pub use oracle::{OracleConfig, OracleDriver};
pub use postgres::{PostgresConfig, PostgresDriver};
pub use redshift::{RedshiftConfig, RedshiftDriver};
pub use snowflake::{SnowflakeConfig, SnowflakeDriver};
pub use type_detection::detect_types_from_tabular;
pub use types::{
    Column, ColumnInfo, CreateTableIndex, DatabaseStructure, DownloadQueryResultsOptions,
    DownloadTableOptions, DownloadedData, DriverCapabilities, ExternalCreateTableOptions,
    GenericType, IndexSql, InlineTable, QueryOptions, QueryResult, Row, SchemaColumn, SchemaName,
    SchemaTable, StreamOptions, StreamTableData, TableCsvData, TableMemoryData, TableName,
    TableStructure,
};
