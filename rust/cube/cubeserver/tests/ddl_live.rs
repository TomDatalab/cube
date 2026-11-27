//! "Create in database" and "Generate SQL" run for real, one database per
//! dialect the tests can reach: each is gated on its variable and skipped
//! without it.
//!
//! * `DDL_LIVE_POSTGRES` — `host:port:user:password:database` of a scratch
//!   database (the test creates and drops schema `ddl_live`);
//! * `DDL_LIVE_MYSQL`, `DDL_LIVE_MSSQL`, `DDL_LIVE_CLICKHOUSE`,
//!   `DDL_LIVE_CUBESTORE` — `host:port:user:password` of a throwaway server;
//! * `DDL_LIVE_FILES=1` — SQLite and DuckDB in temporary files.
//!
//! Each run creates the model's two tables (a parent with an auto-increment
//! key and a unique key, a child with a default, checks, an index and a
//! reference), checks the driver sees them with their columns, inserts rows
//! through the keys, then runs the whole generated script (drops, creates,
//! foreign keys, the view, comments) and reads the view.

use std::collections::HashMap;
use std::sync::Arc;

use cubedriver::{Driver, DriverConfig, DriverFactory, QueryOptions};
use cubeserver::rbac::cube_model::Definition;
use cubeserver::rbac::ddl::{create_plan, script, ScriptOptions};
use serde_json::json;

const SCHEMA: &str = "ddl_live";

fn definition(schema: &str) -> Definition {
    let table = |alias: &str, draft: serde_json::Value, comment: Option<&str>| {
        let mut t = json!({ "alias": alias, "schema": schema, "table": alias, "x": 0.0, "y": 0.0, "draft": draft });
        if let Some(c) = comment {
            t["comment"] = json!(c);
        }
        t
    };
    serde_json::from_value(json!({
        "dataSource": "live",
        "tables": [
            table("ddl_orders", json!({
                "columns": [
                    { "name": "id", "type": "bigint", "primaryKey": true },
                    { "name": "customer_id", "type": "integer" },
                    { "name": "total", "type": "numeric", "default": "0", "check": "total >= 0" },
                    { "name": "created_at", "type": "timestamp", "nullable": true }
                ],
                "indexes": [{ "name": "ix_ddl_created", "columns": [{ "name": "created_at", "desc": true }] }],
                "checks": [{ "name": "ck_ddl_total", "expression": "total < 1000000" }]
            }), None),
            table("ddl_customers", json!({
                "columns": [
                    { "name": "id", "type": "integer", "primaryKey": true, "autoIncrement": true },
                    { "name": "name", "type": "text", "comment": "Full name" },
                    { "name": "email", "type": "varchar(120)", "nullable": true }
                ],
                "uniqueKeys": [{ "name": "uk_ddl_email", "columns": ["email"] }]
            }), Some("Who buys"))
        ],
        "joins": [{
            "from": { "alias": "ddl_orders", "column": "customer_id" },
            "to": { "alias": "ddl_customers", "column": "id" },
            "relationship": "many_to_one",
            "join": "left",
            "onDelete": "cascade"
        }],
        "views": [{
            "id": "v1", "name": "ddl_big_orders", "schema": schema, "comment": "Big orders",
            "sql": format!("SELECT id, total FROM {}ddl_orders WHERE total > 100", qualifier(schema)),
            "columns": [{ "name": "id", "type": "bigint" }, { "name": "total", "type": "numeric" }],
            "dependsOnTables": ["ddl_orders"], "dependsOnViews": [], "x": 0.0, "y": 0.0
        }]
    }))
    .expect("a valid definition")
}

/// `schema.` for the view's query; SQLite's main database needs none.
fn qualifier(schema: &str) -> String {
    if schema == "main" {
        String::new()
    } else {
        format!("{schema}.")
    }
}

fn driver(db_type: &str, env: &[(&str, String)]) -> Arc<dyn Driver> {
    let mut vars: HashMap<String, String> = env.iter().map(|(k, v)| (k.to_string(), v.clone())).collect();
    vars.insert("CUBEJS_DB_TYPE".into(), db_type.into());
    let config = DriverConfig::from_env_source(&vars, None, false).expect("driver config");
    DriverFactory::create_with_env(db_type, config, &vars).expect("driver")
}

async fn run(driver: &Arc<dyn Driver>, sql: &str) {
    driver
        .query(sql, &[], &QueryOptions::default())
        .await
        .unwrap_or_else(|e| panic!("{e}\n{sql}"));
}

async fn rows(driver: &Arc<dyn Driver>, sql: &str) -> usize {
    driver
        .query(sql, &[], &QueryOptions::default())
        .await
        .unwrap_or_else(|e| panic!("{e}\n{sql}"))
        .rows
        .len()
}

/// The statements of a script, split where a line ends one.
fn statements(script: &str) -> Vec<String> {
    script
        .split(";\n")
        .map(|s| {
            s.lines()
                .filter(|l| !l.starts_with("--"))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

struct Case<'a> {
    db_type: &'a str,
    schema: &'a str,
    setup: Vec<String>,
    teardown: Vec<String>,
    /// Inserts through the keys: a customer (its id generated or given), an
    /// order of that customer.
    inserts: Vec<String>,
    /// Whether the generated script can run again over what it created
    /// (its drops come first).
    rerun_script: bool,
}

async fn exercise(case: Case<'_>, d: Arc<dyn Driver>) {
    for sql in &case.setup {
        run(&d, sql).await;
    }
    let def = definition(case.schema);
    let (create, notes) = create_plan(&def, case.db_type).unwrap_or_else(|e| panic!("{}: {e}", case.db_type));
    eprintln!("{}: {} statements, notes: {notes:?}", case.db_type, create.len());
    for s in &create {
        run(&d, &s.sql).await;
    }

    let structure = d.tables_schema().await.expect("the driver lists the tables");
    let tables = structure
        .get(case.schema)
        .unwrap_or_else(|| panic!("{}: no schema {} in {:?}", case.db_type, case.schema, structure.keys()));
    for (table, columns) in [("ddl_customers", 3), ("ddl_orders", 4)] {
        let found = tables
            .get(table)
            .unwrap_or_else(|| panic!("{}: no table {table} in {:?}", case.db_type, tables.keys()));
        // Cube Store adds `__seq` to a table with a unique key.
        let own = found.iter().filter(|c| !c.name.starts_with("__")).count();
        assert_eq!(own, columns, "{}: {table} columns {found:?}", case.db_type);
    }
    for sql in &case.inserts {
        run(&d, sql).await;
    }
    let q = qualifier(case.schema);
    assert_eq!(rows(&d, &format!("SELECT * FROM {q}ddl_orders")).await, 1, "{}", case.db_type);

    if case.rerun_script {
        let opts = ScriptOptions { drop: true, comments: true, ..Default::default() };
        let sql = script(&def, &structure, case.db_type, &opts).unwrap();
        for s in statements(&sql) {
            run(&d, &s).await;
        }
        assert_eq!(rows(&d, &format!("SELECT * FROM {q}ddl_big_orders")).await, 0, "{}", case.db_type);
    }
    for sql in &case.teardown {
        run(&d, sql).await;
    }
    let _ = d.release().await;
}

fn server(var: &str) -> Option<(String, String, String, String)> {
    let v = std::env::var(var).ok().filter(|v| !v.is_empty())?;
    let p: Vec<&str> = v.splitn(4, ':').collect();
    Some((p[0].into(), p[1].into(), p[2].into(), p[3].into()))
}

fn env(host: &str, port: &str, user: &str, pass: &str, db: &str) -> Vec<(&'static str, String)> {
    vec![
        ("CUBEJS_DB_HOST", host.into()),
        ("CUBEJS_DB_PORT", port.into()),
        ("CUBEJS_DB_USER", user.into()),
        ("CUBEJS_DB_PASS", pass.into()),
        ("CUBEJS_DB_NAME", db.into()),
    ]
}

#[tokio::test]
async fn postgres() {
    let Some(v) = std::env::var("DDL_LIVE_POSTGRES").ok().filter(|v| !v.is_empty()) else {
        return eprintln!("skipped: DDL_LIVE_POSTGRES");
    };
    let p: Vec<&str> = v.splitn(5, ':').collect();
    let d = driver("postgres", &env(p[0], p[1], p[2], p[3], p[4]));
    exercise(
        Case {
            db_type: "postgres",
            schema: SCHEMA,
            setup: vec![format!("DROP SCHEMA IF EXISTS {SCHEMA} CASCADE"), format!("CREATE SCHEMA {SCHEMA}")],
            teardown: vec![format!("DROP SCHEMA {SCHEMA} CASCADE")],
            inserts: vec![
                format!("INSERT INTO {SCHEMA}.ddl_customers (name, email) VALUES ('Ana', 'a@x')"),
                format!("INSERT INTO {SCHEMA}.ddl_orders (id, customer_id) SELECT 1, id FROM {SCHEMA}.ddl_customers"),
            ],
            rerun_script: true,
        },
        d,
    )
    .await;
}

#[tokio::test]
async fn mysql() {
    let Some((h, port, u, pw)) = server("DDL_LIVE_MYSQL") else {
        return eprintln!("skipped: DDL_LIVE_MYSQL");
    };
    // MySQL lists the tables of the connection's database: make it, then
    // connect to it.
    let admin = driver("mysql", &env(&h, &port, &u, &pw, "mysql"));
    run(&admin, &format!("DROP DATABASE IF EXISTS {SCHEMA}")).await;
    run(&admin, &format!("CREATE DATABASE {SCHEMA}")).await;
    let d = driver("mysql", &env(&h, &port, &u, &pw, SCHEMA));
    exercise(
        Case {
            db_type: "mysql",
            schema: SCHEMA,
            setup: vec![],
            teardown: vec![format!("DROP DATABASE {SCHEMA}")],
            inserts: vec![
                format!("INSERT INTO {SCHEMA}.ddl_customers (name, email) VALUES ('Ana', 'a@x')"),
                format!("INSERT INTO {SCHEMA}.ddl_orders (id, customer_id) SELECT 1, id FROM {SCHEMA}.ddl_customers"),
            ],
            rerun_script: true,
        },
        d,
    )
    .await;
}

#[tokio::test]
async fn mssql() {
    let Some((h, port, u, pw)) = server("DDL_LIVE_MSSQL") else {
        return eprintln!("skipped: DDL_LIVE_MSSQL");
    };
    let d = driver("mssql", &env(&h, &port, &u, &pw, "master"));
    exercise(
        Case {
            db_type: "mssql",
            schema: SCHEMA,
            setup: vec![
                format!("IF OBJECT_ID('{SCHEMA}.ddl_big_orders', 'V') IS NOT NULL DROP VIEW {SCHEMA}.ddl_big_orders"),
                format!("IF OBJECT_ID('{SCHEMA}.ddl_orders', 'U') IS NOT NULL DROP TABLE {SCHEMA}.ddl_orders"),
                format!("IF OBJECT_ID('{SCHEMA}.ddl_customers', 'U') IS NOT NULL DROP TABLE {SCHEMA}.ddl_customers"),
                format!("IF SCHEMA_ID('{SCHEMA}') IS NULL EXEC('CREATE SCHEMA {SCHEMA}')"),
            ],
            teardown: vec![
                format!("DROP VIEW {SCHEMA}.ddl_big_orders"),
                format!("DROP TABLE {SCHEMA}.ddl_orders"),
                format!("DROP TABLE {SCHEMA}.ddl_customers"),
                format!("DROP SCHEMA {SCHEMA}"),
            ],
            inserts: vec![
                format!("INSERT INTO {SCHEMA}.ddl_customers (name, email) VALUES ('Ana', 'a@x')"),
                format!("INSERT INTO {SCHEMA}.ddl_orders (id, customer_id) SELECT 1, id FROM {SCHEMA}.ddl_customers"),
            ],
            rerun_script: true,
        },
        d,
    )
    .await;
}

#[tokio::test]
async fn clickhouse() {
    let Some((h, port, u, pw)) = server("DDL_LIVE_CLICKHOUSE") else {
        return eprintln!("skipped: DDL_LIVE_CLICKHOUSE");
    };
    let d = driver("clickhouse", &env(&h, &port, &u, &pw, "default"));
    exercise(
        Case {
            db_type: "clickhouse",
            schema: SCHEMA,
            setup: vec![format!("DROP DATABASE IF EXISTS {SCHEMA}"), format!("CREATE DATABASE {SCHEMA}")],
            teardown: vec![format!("DROP DATABASE {SCHEMA}")],
            // No auto increment: the key is given.
            inserts: vec![
                format!("INSERT INTO {SCHEMA}.ddl_customers (id, name, email) VALUES (1, 'Ana', 'a@x')"),
                format!("INSERT INTO {SCHEMA}.ddl_orders (id, customer_id) VALUES (1, 1)"),
            ],
            rerun_script: true,
        },
        d,
    )
    .await;
}

#[tokio::test]
async fn cubestore() {
    let Some((h, port, u, pw)) = server("DDL_LIVE_CUBESTORE") else {
        return eprintln!("skipped: DDL_LIVE_CUBESTORE");
    };
    // Its factory reads the process environment.
    std::env::set_var("CUBEJS_CUBESTORE_HOST", &h);
    std::env::set_var("CUBEJS_CUBESTORE_PORT", &port);
    let d = driver(
        "cubestore",
        &[("CUBEJS_CUBESTORE_HOST", h.clone()), ("CUBEJS_CUBESTORE_PORT", port.clone()), ("CUBEJS_CUBESTORE_USER", u.clone()), ("CUBEJS_CUBESTORE_PASS", pw.clone())],
    );
    exercise(
        Case {
            db_type: "cubestore",
            schema: SCHEMA,
            setup: vec![format!("CREATE SCHEMA IF NOT EXISTS {SCHEMA}")],
            teardown: vec![
                format!("DROP TABLE {SCHEMA}.ddl_orders"),
                format!("DROP TABLE {SCHEMA}.ddl_customers"),
            ],
            inserts: vec![
                format!("INSERT INTO {SCHEMA}.ddl_customers (id, name, email) VALUES (1, 'Ana', 'a@x')"),
                format!("INSERT INTO {SCHEMA}.ddl_orders (id, customer_id, total, created_at) VALUES (1, 1, 0, '2026-01-01T00:00:00.000Z')"),
            ],
            // Cube Store has no views: the script's create part is what Create runs.
            rerun_script: false,
        },
        d,
    )
    .await;
}

#[tokio::test]
async fn sqlite_and_duckdb() {
    if std::env::var("DDL_LIVE_FILES").is_err() {
        return eprintln!("skipped: DDL_LIVE_FILES");
    }
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("live.db");
    let d = driver("sqlite", &[("CUBEJS_DB_NAME", file.display().to_string())]);
    exercise(
        Case {
            db_type: "sqlite",
            schema: "main",
            setup: vec!["PRAGMA foreign_keys = ON".into()],
            teardown: vec![],
            inserts: vec![
                "INSERT INTO ddl_customers (name, email) VALUES ('Ana', 'a@x')".into(),
                "INSERT INTO ddl_orders (id, customer_id) SELECT 1, id FROM ddl_customers".into(),
            ],
            rerun_script: true,
        },
        d,
    )
    .await;

    let file = dir.path().join("live.duckdb");
    let d = driver("duckdb", &[("CUBEJS_DB_DUCKDB_DATABASE_PATH", file.display().to_string())]);
    exercise(
        Case {
            db_type: "duckdb",
            schema: SCHEMA,
            setup: vec![format!("CREATE SCHEMA IF NOT EXISTS {SCHEMA}")],
            teardown: vec![],
            inserts: vec![
                format!("INSERT INTO {SCHEMA}.ddl_customers (name, email) VALUES ('Ana', 'a@x')"),
                format!("INSERT INTO {SCHEMA}.ddl_orders (id, customer_id) SELECT 1, id FROM {SCHEMA}.ddl_customers"),
            ],
            // DuckDB cannot drop a table another references: the creates ran.
            rerun_script: false,
        },
        d,
    )
    .await;
}
