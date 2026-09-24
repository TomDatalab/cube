//! One planning snapshot per dialect for a grouped query and for a granular
//! time dimension: between them they exercise the quoting, the parameter
//! placeholder, `convertTz`, `timeGroupedColumn` and the time-series
//! templates each `BaseQuery` subclass overrides.

use cubeplanner::{
    plan, Dialect, Model, PlanOptions, PlannedSql, PlannerError, PlannerQuery, DB_TYPE_DIALECTS,
};
use serde_json::json;
use std::str::FromStr;

fn test_model() -> Model {
    Model::from_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/model")).unwrap()
}

fn plan_with(dialect: Dialect, value: serde_json::Value) -> PlannedSql {
    let query = PlannerQuery::from_value(value).unwrap();
    plan(
        &test_model(),
        &query,
        &PlanOptions::postgres().with_dialect(dialect),
    )
    .unwrap_or_else(|e| panic!("{dialect}: {e}"))
}

/// A grouped query with a filter: identifier quoting, the parameter
/// placeholder and the join.
fn grouped(dialect: Dialect) -> PlannedSql {
    plan_with(
        dialect,
        json!({
            "measures": ["orders.total_amount"],
            "dimensions": ["users.city"],
            "filters": [{
                "member": "orders.status",
                "operator": "equals",
                "values": ["shipped"]
            }],
            "order": [{ "id": "orders.total_amount", "desc": true }],
            "limit": 10
        }),
    )
}

/// A time dimension grouped by month over a date range: `convertTz`,
/// `timeGroupedColumn` and the timestamp casts.
fn by_month(dialect: Dialect) -> PlannedSql {
    plan_with(
        dialect,
        json!({
            "measures": ["orders.count"],
            "timeDimensions": [{
                "dimension": "orders.created_at",
                "granularity": "month",
                "dateRange": ["2024-01-01", "2024-03-31"]
            }]
        }),
    )
}

macro_rules! dialect_snapshots {
    ($dialect:ident, $grouped:ident, $by_month:ident) => {
        #[test]
        fn $grouped() {
            insta::assert_snapshot!(grouped(Dialect::$dialect).sql);
        }

        #[test]
        fn $by_month() {
            insta::assert_snapshot!(by_month(Dialect::$dialect).sql);
        }
    };
}

dialect_snapshots!(MySql, mysql_grouped, mysql_by_month);
dialect_snapshots!(ClickHouse, clickhouse_grouped, clickhouse_by_month);
dialect_snapshots!(BigQuery, bigquery_grouped, bigquery_by_month);
dialect_snapshots!(Snowflake, snowflake_grouped, snowflake_by_month);
dialect_snapshots!(Databricks, databricks_grouped, databricks_by_month);
dialect_snapshots!(MsSql, mssql_grouped, mssql_by_month);
dialect_snapshots!(Redshift, redshift_grouped, redshift_by_month);
dialect_snapshots!(MongoBi, mongobi_grouped, mongobi_by_month);
dialect_snapshots!(Presto, presto_grouped, presto_by_month);
dialect_snapshots!(Trino, trino_grouped, trino_by_month);
dialect_snapshots!(Vertica, vertica_grouped, vertica_by_month);
dialect_snapshots!(Crate, crate_grouped, crate_by_month);
dialect_snapshots!(Hive, hive_grouped, hive_by_month);
dialect_snapshots!(Oracle, oracle_grouped, oracle_by_month);
dialect_snapshots!(Sqlite, sqlite_grouped, sqlite_by_month);
dialect_snapshots!(Druid, druid_grouped, druid_by_month);
dialect_snapshots!(Firebolt, firebolt_grouped, firebolt_by_month);
dialect_snapshots!(Dremio, dremio_grouped, dremio_by_month);
dialect_snapshots!(Ksql, ksql_grouped, ksql_by_month);
dialect_snapshots!(QuestDb, questdb_grouped, questdb_by_month);
dialect_snapshots!(Pinot, pinot_grouped, pinot_by_month);
dialect_snapshots!(DuckDb, duckdb_grouped, duckdb_by_month);

#[test]
fn every_dialect_quotes_and_parameterizes_the_way_its_driver_does() {
    // (dialect, identifier quote, first parameter placeholder)
    for (dialect, quote, param) in [
        (Dialect::Postgres, '"', "$1"),
        (Dialect::CubeStore, '"', "?"),
        (Dialect::MySql, '`', "?"),
        (Dialect::ClickHouse, '`', "?"),
        (Dialect::BigQuery, '`', "?"),
        (Dialect::Snowflake, '"', "?"),
        (Dialect::Databricks, '`', "?"),
        (Dialect::MsSql, '"', "@_1"),
        (Dialect::Redshift, '"', "$1"),
        (Dialect::MongoBi, '`', "?"),
        (Dialect::Presto, '"', "?"),
        (Dialect::Trino, '"', "?"),
        (Dialect::Vertica, '"', "?"),
        (Dialect::Crate, '"', "$1"),
        (Dialect::Hive, '`', "?"),
        (Dialect::Oracle, '"', "?"),
        (Dialect::Sqlite, '"', "?"),
        (Dialect::Druid, '"', "?"),
        (Dialect::Firebolt, '"', "?"),
        (Dialect::Dremio, '"', "?"),
        (Dialect::Ksql, '`', "?"),
        (Dialect::QuestDb, '"', "$1"),
        (Dialect::Pinot, '"', "?"),
        (Dialect::DuckDb, '"', "?"),
    ] {
        let sql = grouped(dialect).sql;
        assert!(
            sql.contains(&format!("{quote}orders{quote}")),
            "{dialect} should quote identifiers with {quote}: {sql}"
        );
        assert!(
            sql.contains(param),
            "{dialect} should render {param} placeholders: {sql}"
        );
    }
}

#[test]
fn time_grouping_uses_each_dialects_own_expression() {
    for (dialect, needle) in [
        (Dialect::Postgres, "date_trunc('month'"),
        (Dialect::MySql, "DATE_FORMAT("),
        (Dialect::ClickHouse, "toStartOfMonth("),
        (Dialect::BigQuery, "DATETIME_TRUNC("),
        (Dialect::Snowflake, "date_trunc('MONTH'"),
        (Dialect::Databricks, "date_trunc('month'"),
        (Dialect::MsSql, "dateadd(month, DATEDIFF(month, 0,"),
        (Dialect::Redshift, "date_trunc('month'"),
        (Dialect::MongoBi, "DATE_FORMAT("),
        (Dialect::Presto, "date_trunc('month'"),
        (Dialect::Trino, "date_trunc('month'"),
        (Dialect::Vertica, "TRUNC("),
        (Dialect::Crate, "date_trunc('month'"),
        (Dialect::Hive, "DATE_FORMAT("),
        (Dialect::Oracle, "TRUNC("),
        (Dialect::Sqlite, "strftime('%Y-%m-01T00:00:00.000'"),
        (Dialect::Druid, "DATE_TRUNC('month'"),
        (Dialect::Firebolt, "DATE_TRUNC('MONTH'"),
        (Dialect::Dremio, "DATE_TRUNC('month'"),
        (Dialect::Ksql, "FORMAT_TIMESTAMP("),
        (Dialect::QuestDb, "timestamp_floor('M'"),
        (Dialect::Pinot, "dateTrunc('month'"),
        (Dialect::DuckDb, "DATE_TRUNC('month'"),
    ] {
        let sql = by_month(dialect).sql;
        assert!(
            sql.contains(needle),
            "{dialect} should group by month with `{needle}`: {sql}"
        );
    }
}

#[test]
fn convert_tz_is_the_dialects_own() {
    for (dialect, needle) in [
        (Dialect::Postgres, "AT TIME ZONE 'America/Los_Angeles'"),
        // `CUBEJS_DB_MYSQL_USE_NAMED_TIMEZONES` is off by default, so MySQL
        // and MS SQL interpolate the zone's offset rather than its name.
        (Dialect::MySql, "CONVERT_TZ("),
        (Dialect::ClickHouse, "toTimeZone(toDateTime64("),
        (Dialect::BigQuery, "TIMESTAMP(DATETIME("),
        (Dialect::Snowflake, "CONVERT_TIMEZONE('America/Los_Angeles'"),
        (Dialect::Databricks, "from_utc_timestamp("),
        (Dialect::MsSql, "SWITCHOFFSET(TODATETIMEOFFSET("),
        (Dialect::Redshift, "AT TIME ZONE 'America/Los_Angeles'"),
        (Dialect::MongoBi, "TIMESTAMPADD(HOUR, -"),
        (Dialect::Presto, "timezone_hour("),
        (
            Dialect::Trino,
            "AT TIME ZONE 'America/Los_Angeles') AS TIMESTAMP)",
        ),
        (Dialect::Vertica, "AT TIME ZONE 'America/Los_Angeles'"),
        (Dialect::Crate, "AT TIME ZONE 'America/Los_Angeles'"),
        (Dialect::Hive, "from_utc_timestamp("),
        // `OracleQuery.convertTz` leaves the column as it is.
        (Dialect::Oracle, "TRUNC(\"orders\".created_at, 'DD')"),
        (Dialect::Sqlite, "|| '+0"),
        (Dialect::Druid, "TIME_FORMAT("),
        (Dialect::Firebolt, "AT TIME ZONE 'America/Los_Angeles'"),
        (Dialect::Dremio, "CONVERT_TIMEZONE('America/Los_Angeles'"),
        (Dialect::Ksql, "CONVERT_TZ("),
        (Dialect::QuestDb, "to_timezone("),
        (Dialect::Pinot, "toDateTime("),
        (Dialect::DuckDb, "timezone('America/Los_Angeles'"),
    ] {
        let planned = plan_with(
            dialect,
            json!({
                "measures": ["orders.count"],
                "timeDimensions": [{ "dimension": "orders.created_at", "granularity": "day" }],
                "timezone": "America/Los_Angeles"
            }),
        );
        assert!(
            planned.sql.contains(needle),
            "{dialect} should convert the zone with `{needle}`: {}",
            planned.sql
        );
    }
}

/// A custom granularity with an origin goes through `dateBin`, which every
/// dialect spells differently.
#[test]
fn custom_granularity_uses_each_dialects_date_bin() {
    for (dialect, needle) in [
        (Dialect::Postgres, "EXTRACT(EPOCH FROM INTERVAL"),
        (Dialect::MySql, "TIMESTAMPADD(MONTH,"),
        (Dialect::ClickHouse, "date_add(month,"),
        (Dialect::BigQuery, "DATETIME_DIFF("),
        (Dialect::Snowflake, "DATEADD(month,"),
        (Dialect::Databricks, "date_diff(MONTH,"),
        (Dialect::MsSql, "DATEADD(month,"),
        (Dialect::Redshift, "DATEADD(\n      month,"),
        (Dialect::MongoBi, "TIMESTAMPADD(MONTH,"),
        (Dialect::Presto, "date_add('month',"),
        (Dialect::Trino, "date_diff('month',"),
        (Dialect::Crate, "EXTRACT(EPOCH FROM INTERVAL '6 months')"),
        (
            Dialect::Oracle,
            "ADD_MONTHS(TO_TIMESTAMP('2024-01-01T00:00:00.000'",
        ),
        (Dialect::QuestDb, "timestamp_floor('6M',"),
        (Dialect::Pinot, "TIMESTAMPADD(MONTH,"),
        (Dialect::DuckDb, "date_diff('month',"),
    ] {
        let planned = plan_with(
            dialect,
            json!({
                "measures": ["orders.count"],
                "timeDimensions": [{
                    "dimension": "orders.created_at",
                    "granularity": "half_year"
                }]
            }),
        );
        assert!(
            planned.sql.contains(needle),
            "{dialect} should bin a custom granularity with `{needle}`: {}",
            planned.sql
        );
    }
}

/// `BaseQuery.dateBin` throws, and these dialects do not override it: a
/// custom granularity is refused with a named error rather than planned.
#[test]
fn custom_granularity_is_refused_where_the_dialect_has_no_date_bin() {
    for dialect in [
        Dialect::Vertica,
        Dialect::Hive,
        Dialect::Sqlite,
        Dialect::Druid,
        Dialect::Firebolt,
        Dialect::Dremio,
        Dialect::Ksql,
    ] {
        let query = PlannerQuery::from_value(json!({
            "measures": ["orders.count"],
            "timeDimensions": [{
                "dimension": "orders.created_at",
                "granularity": "half_year"
            }]
        }))
        .unwrap();
        let err = plan(
            &test_model(),
            &query,
            &PlanOptions::postgres().with_dialect(dialect),
        )
        .unwrap_err();
        assert!(
            matches!(err, PlannerError::Unsupported(_)),
            "{dialect}: {err:?}"
        );
        assert!(
            err.message()
                .contains(&format!("Unsupported by the {} dialect", dialect.as_str())),
            "{dialect}: {err}"
        );
    }
}

#[test]
fn dialect_names_are_accepted_the_way_a_driver_spells_them() {
    for (name, expected) in [
        ("postgres", Dialect::Postgres),
        ("PostgreSQL", Dialect::Postgres),
        ("cube_store", Dialect::CubeStore),
        ("mysql", Dialect::MySql),
        ("clickhouse", Dialect::ClickHouse),
        ("bigquery", Dialect::BigQuery),
        ("snowflake", Dialect::Snowflake),
        ("databricks", Dialect::Databricks),
        ("mssql", Dialect::MsSql),
        ("sqlserver", Dialect::MsSql),
        ("redshift", Dialect::Redshift),
        ("mongobi", Dialect::MongoBi),
        ("prestodb", Dialect::Presto),
        ("athena", Dialect::Presto),
        ("trino", Dialect::Trino),
        ("vertica", Dialect::Vertica),
        ("crate", Dialect::Crate),
        ("hive", Dialect::Hive),
        ("oracle", Dialect::Oracle),
        ("sqlite", Dialect::Sqlite),
        ("druid", Dialect::Druid),
        ("firebolt", Dialect::Firebolt),
        ("dremio", Dialect::Dremio),
        ("ksql", Dialect::Ksql),
        ("questdb", Dialect::QuestDb),
        ("pinot", Dialect::Pinot),
        ("duckdb", Dialect::DuckDb),
    ] {
        assert_eq!(Dialect::from_str(name).unwrap(), expected, "{name}");
    }

    let err = Dialect::from_str("elasticsearch").unwrap_err();
    assert!(err.message().contains("Unknown SQL dialect"), "{err}");
    assert!(err.message().contains("databricks"), "{err}");
}

/// The dialect is part of the request, so it round-trips through JSON the way
/// the gateway sends it.
#[test]
fn dialect_deserializes_from_the_request() {
    for name in [
        "postgres",
        "cube_store",
        "cubestore",
        "my_sql",
        "mysql",
        "click_house",
        "clickhouse",
        "big_query",
        "bigquery",
        "snowflake",
        "databricks",
        "ms_sql",
        "mssql",
        "sqlserver",
        "redshift",
        "mongo_bi",
        "mongobi",
        "presto",
        "prestodb",
        "athena",
        "trino",
        "vertica",
        "crate",
        "hive",
        "oracle",
        "sqlite",
        "druid",
        "firebolt",
        "dremio",
        "ksql",
        "quest_db",
        "questdb",
        "pinot",
        "duck_db",
        "duckdb",
    ] {
        let options: PlanOptions = serde_json::from_value(json!({ "dialect": name }))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(options.dialect, Dialect::from_str(name).unwrap(), "{name}");
    }
}

/// A caller that has to know which templates a dialect defines — the SQL API
/// deciding what it may push down — enumerates them rather than keeping its
/// own list.
#[test]
fn a_dialects_templates_can_be_enumerated() {
    for dialect in Dialect::ALL {
        let templates = dialect.templates();
        let names: Vec<&str> = templates.template_names().collect();

        assert!(
            names.len() > 100,
            "{dialect} defines only {} templates",
            names.len()
        );
        for required in ["params/param", "quotes/identifiers", "statements/select"] {
            assert!(names.contains(&required), "{dialect} is missing {required}");
        }
        assert_eq!(templates.templates_map().len(), names.len());
    }

    // The deltas really differ from the base set.
    let base = cubeplanner::base_templates();
    let mysql = Dialect::MySql.templates();
    assert_eq!(
        base.templates_map().get("quotes/identifiers").unwrap(),
        "\""
    );
    assert_eq!(
        mysql.templates_map().get("quotes/identifiers").unwrap(),
        "`"
    );
    // MySQL deletes `expressions/ilike`, so the key is gone rather than empty.
    assert!(base.templates_map().contains_key("expressions/ilike"));
    assert!(!mysql.templates_map().contains_key("expressions/ilike"));
}

/// Every `CUBEJS_DB_TYPE` Cube ships a driver for plans in its own dialect —
/// `ADAPTERS` in `QueryBuilder.ts` plus the drivers bundling a `BaseQuery`.
#[test]
fn every_db_type_maps_to_its_dialect() {
    for (db_type, expected) in [
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
        // The configuration is read case-insensitively.
        ("Redshift", Dialect::Redshift),
        (" snowflake ", Dialect::Snowflake),
    ] {
        assert_eq!(
            Dialect::for_db_type(db_type).unwrap(),
            expected,
            "{db_type}"
        );
    }

    // The published table and the lookup agree, and every dialect but
    // CubeStore's own is reachable from some database type.
    for (db_type, dialect) in DB_TYPE_DIALECTS {
        assert_eq!(Dialect::for_db_type(db_type).unwrap(), dialect);
    }
    for dialect in Dialect::ALL {
        assert!(
            DB_TYPE_DIALECTS.iter().any(|(_, d)| *d == dialect),
            "{dialect} is not reachable from any CUBEJS_DB_TYPE"
        );
    }
}

/// An unknown data source type is an error that names it, never a silent
/// fallback to Postgres.
#[test]
fn an_unknown_db_type_is_refused_by_name() {
    for db_type in ["elasticsearch", "postgresql", "", "my-sql"] {
        let err = Dialect::for_db_type(db_type).unwrap_err();
        assert!(matches!(err, PlannerError::Unsupported(_)), "{err:?}");
        assert!(
            err.message().contains(&format!("`{db_type}`")),
            "{db_type}: {err}"
        );
        assert!(err.message().contains("databricks-jdbc"), "{err}");
    }
}
