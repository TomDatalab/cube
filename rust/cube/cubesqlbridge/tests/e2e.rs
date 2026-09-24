//! End to end: a real Postgres client against the SQL API, with nothing but
//! Rust behind it.
//!
//! The server is started on an ephemeral port with a small YAML model, a
//! [`QueryExecutor`] that answers with fixed rows, and no JavaScript anywhere
//! in the path. A `tokio-postgres` client then connects and runs the three
//! kinds of statement a BI tool sends: a constant, a catalog query and a real
//! query against the model.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cubesql::config::processing_loop::ShutdownMode;
use cubesql::CubeError;
use cubesqlbridge::{
    start_sql_api, LoadResponse, LoadResult, LoadResultDataColumnar, ModelSource, QueryExecutor,
    SqlApi, SqlApiConfig, SqlAuthConfig,
};
use serde_json::{json, Value};
use tokio::net::TcpStream;

const MODEL: &str = include_str!("model.yml");
const SQL_USER: &str = "cube";
const SQL_PASSWORD: &str = "secret";

/// A `QueryExecutor` that answers every query with one row per member.
///
/// It reads the members out of the request it is given, so whatever the SQL
/// API decides to ask for lines up with what comes back, and it records every
/// request so a test can assert on what the SQL API actually sent.
#[derive(Debug, Default)]
struct FakeExecutor {
    calls: Mutex<Vec<Value>>,
    rows: AtomicUsize,
}

impl FakeExecutor {
    fn with_rows(rows: usize) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            rows: AtomicUsize::new(rows),
        }
    }

    fn last_request(&self) -> Option<Value> {
        self.calls.lock().expect("not poisoned").last().cloned()
    }

    fn call_count(&self) -> usize {
        self.calls.lock().expect("not poisoned").len()
    }

    /// A value of the right JSON shape for one member: measures are counted,
    /// dimensions are named.
    fn value_for(member: &str, is_measure: bool, row: usize) -> Value {
        if is_measure {
            json!(10 * (row as i64 + 1))
        } else {
            json!(format!("{}-{}", member.replace('.', "_"), row))
        }
    }
}

#[async_trait::async_trait]
impl QueryExecutor for FakeExecutor {
    async fn execute(
        &self,
        query: Value,
        _security_context: &Value,
    ) -> Result<LoadResponse, CubeError> {
        self.calls.lock().expect("not poisoned").push(query.clone());

        let inner = query.get("query").cloned().unwrap_or(Value::Null);
        let names = |key: &str| -> Vec<String> {
            inner
                .get(key)
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        };

        let measures = names("measures");
        let dimensions = names("dimensions");
        let rows = self.rows.load(Ordering::Relaxed);

        let members: Vec<String> = measures.iter().chain(dimensions.iter()).cloned().collect();
        let columns: Vec<Vec<Value>> = members
            .iter()
            .map(|member| {
                let is_measure = measures.contains(member);
                (0..rows)
                    .map(|row| Self::value_for(member, is_measure, row))
                    .collect()
            })
            .collect();

        Ok(LoadResponse {
            results: vec![LoadResult {
                data_source: Some("default".to_string()),
                annotation: Box::default(),
                data: LoadResultDataColumnar { members, columns },
                refresh_key_values: None,
                last_refresh_time: None,
                external: Some(false),
                used_pre_aggregations: None,
            }],
            ..Default::default()
        })
    }
}

struct Running {
    api: SqlApi,
    port: u16,
    executor: Arc<FakeExecutor>,
}

impl Running {
    async fn stop(self) {
        let _ = self.api.stop_processing_loops(ShutdownMode::Fast).await;
    }
}

async fn start(executor: Arc<FakeExecutor>) -> Running {
    let port = portpicker::pick_unused_port().expect("an ephemeral port");

    let config = SqlApiConfig {
        model_source: ModelSource::yaml(MODEL),
        postgres_bind_address: Some(format!("127.0.0.1:{port}")),
        planner_threads: 2,
        dialects: Default::default(),
        include_hidden_members: false,
        executor: executor.clone(),
        auth_config: SqlAuthConfig {
            sql_user: Some(SQL_USER.to_string()),
            sql_password: Some(SQL_PASSWORD.to_string()),
            sql_super_user: Some("admin".to_string()),
            dev_mode: false,
        },
        authenticator: None,
    };

    let api = start_sql_api(config)
        .await
        .expect("the SQL API should start");
    api.spawn_processing_loops()
        .await
        .expect("the Postgres listener should start");

    // The listener binds inside the spawned loop, so wait for it to accept.
    for attempt in 0..200 {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            break;
        }
        assert!(attempt < 199, "the SQL API never started listening");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    Running {
        api,
        port,
        executor,
    }
}

async fn connect(port: u16) -> tokio_postgres::Client {
    connect_as(port, SQL_USER, SQL_PASSWORD)
        .await
        .expect("the client should connect")
}

async fn connect_as(
    port: u16,
    user: &str,
    password: &str,
) -> Result<tokio_postgres::Client, tokio_postgres::Error> {
    let (client, connection) = tokio_postgres::connect(
        &format!("host=127.0.0.1 port={port} user={user} password={password} dbname=db"),
        tokio_postgres::NoTls,
    )
    .await?;

    tokio::spawn(async move {
        let _ = connection.await;
    });

    Ok(client)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_client_connects_and_runs_a_constant() {
    let running = start(Arc::new(FakeExecutor::with_rows(0))).await;
    let client = connect(running.port).await;

    let rows = client
        .query("SELECT 1", &[])
        .await
        .expect("SELECT 1 should run");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<_, i64>(0), 1);

    // Nothing that does not read the model reaches the executor.
    assert_eq!(running.executor.call_count(), 0);

    running.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_wrong_password_is_refused() {
    let running = start(Arc::new(FakeExecutor::with_rows(0))).await;

    for (user, password) in [(SQL_USER, "not-the-password"), ("nobody", SQL_PASSWORD)] {
        let error = connect_as(running.port, user, password)
            .await
            .expect_err("the connection should be refused");
        let message = error
            .as_db_error()
            .map(|db| db.message().to_string())
            .unwrap_or_else(|| error.to_string());
        assert!(
            message.contains("password authentication failed"),
            "unexpected error for {user}: {message}"
        );
    }

    running.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_catalog_lists_the_cubes_of_the_model() {
    let running = start(Arc::new(FakeExecutor::with_rows(0))).await;
    let client = connect(running.port).await;

    // The `SHOW TABLES` a BI tool runs on connect; over the Postgres protocol
    // that is the information_schema view cubesql serves from its meta.
    let rows = client
        .query(
            "SELECT table_name FROM information_schema.tables WHERE table_schema = 'public'",
            &[],
        )
        .await
        .expect("the catalog query should run");

    let tables: HashSet<String> = rows.iter().map(|row| row.get::<_, String>(0)).collect();
    for expected in ["orders", "users", "sales"] {
        assert!(
            tables.contains(expected),
            "{expected} is missing from {tables:?}"
        );
    }

    // The columns of a cube are its members.
    let rows = client
        .query(
            "SELECT column_name FROM information_schema.columns WHERE table_name = 'orders'",
            &[],
        )
        .await
        .expect("the column catalog query should run");
    let columns: HashSet<String> = rows.iter().map(|row| row.get::<_, String>(0)).collect();
    assert!(columns.contains("status"), "{columns:?}");
    assert!(columns.contains("count"), "{columns:?}");

    assert_eq!(running.executor.call_count(), 0);

    running.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_query_reaches_the_executor_and_comes_back_as_rows() {
    let executor = Arc::new(FakeExecutor::with_rows(2));
    let running = start(executor.clone()).await;
    let client = connect(running.port).await;

    let rows = client
        .query(
            "SELECT status, MEASURE(count) AS orders FROM orders GROUP BY 1 ORDER BY 1",
            &[],
        )
        .await
        .expect("the query should run");

    assert_eq!(rows.len(), 2, "the executor returned two rows");
    assert_eq!(rows[0].get::<_, String>(0), "orders_status-0");
    assert_eq!(rows[1].get::<_, String>(0), "orders_status-1");
    assert_eq!(rows[0].get::<_, i64>(1), 10);
    assert_eq!(rows[1].get::<_, i64>(1), 20);

    // The SQL API asked for exactly the members of the statement, and the
    // request carried the session it ran under.
    assert_eq!(executor.call_count(), 1);
    let request = executor.last_request().expect("one request");
    assert_eq!(
        request["query"]["measures"],
        json!(["orders.count"]),
        "{request}"
    );
    assert_eq!(
        request["query"]["dimensions"],
        json!(["orders.status"]),
        "{request}"
    );
    assert_eq!(request["session"]["user"], json!(SQL_USER), "{request}");
    assert_eq!(request["streaming"], json!(false), "{request}");
    assert!(
        request["request"]["id"]
            .as_str()
            .is_some_and(|id| id.contains("-span-")),
        "{request}"
    );

    running.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_query_against_a_view_spans_its_cubes() {
    let executor = Arc::new(FakeExecutor::with_rows(1));
    let running = start(executor.clone()).await;
    let client = connect(running.port).await;

    let rows = client
        .query("SELECT MEASURE(count) AS orders FROM sales", &[])
        .await
        .expect("the view query should run");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<_, i64>(0), 10);

    let request = executor.last_request().expect("one request");
    assert_eq!(request["query"]["measures"], json!(["sales.count"]));

    running.stop().await;
}
