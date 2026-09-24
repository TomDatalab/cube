#![cfg(feature = "ksql")]
//! Integration tests for [`KsqlDriver`].
//!
//! They run only when `CUBEJS_TEST_KSQL_URL` is set. The Kafka and Cube Store
//! parts need more variables:
//!
//! ```text
//! CUBEJS_TEST_KSQL_URL=http://127.0.0.1:16788          # ksqlDB, from the tests
//! CUBEJS_TEST_KSQL_KAFKA_HOST=127.0.0.1:16792          # Kafka, from the tests
//! CUBEJS_TEST_KSQL_CUBESTORE_URL=ws://127.0.0.1:16730  # Cube Store, from the tests
//! CUBEJS_TEST_KSQL_URL_FOR_CUBESTORE=http://drv-dremio-ksqldb:8088  # ksqlDB, from Cube Store
//! CUBEJS_TEST_KSQL_KAFKA_FOR_CUBESTORE=kafka:9092                   # Kafka, from Cube Store
//! ```

use std::time::Duration;

use cubedriver::ksql::{StreamingSource, STREAM_OFFSET_OPTION};
use cubedriver::{
    Column, CubeStoreConfig, CubeStoreDriver, Driver, DriverConfig, ExternalCreateTableOptions,
    KsqlConfig, KsqlDriver, QueryOptions,
};
use serde_json::{json, Value};

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}

/// ksqlDB runs one DDL statement at a time (concurrent ones fence each
/// other's command producer), which is why the driver's concurrency is 1.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

macro_rules! require_ksql {
    () => {
        match env("CUBEJS_TEST_KSQL_URL") {
            Some(url) => (url, SERIAL.lock().await),
            None => {
                eprintln!("CUBEJS_TEST_KSQL_URL is not set, skipping");
                return;
            }
        }
    };
}

fn config(url: &str) -> KsqlConfig {
    let mut driver = DriverConfig::default();
    driver.data_source.url = Some(url.to_string());
    KsqlConfig::from_driver_config(driver)
}

fn driver(url: &str) -> KsqlDriver {
    KsqlDriver::new(config(url)).unwrap()
}

fn offset(offset: &str) -> QueryOptions {
    let mut options = QueryOptions::default();
    options
        .extra
        .insert(STREAM_OFFSET_OPTION.to_string(), Value::from(offset));
    options
}

async fn exec(driver: &KsqlDriver, sql: &str) {
    driver
        .query(sql, &[], &QueryOptions::default())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"));
}

/// Creates the `ORDERS_S` source stream (2 partitions) with three rows.
async fn orders_stream(driver: &KsqlDriver) {
    static ONCE: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
    ONCE.get_or_init(|| async {
        exec(
            driver,
            "CREATE STREAM IF NOT EXISTS ORDERS_S (ID STRING KEY, AMOUNT DOUBLE, STATUS STRING) \
             WITH (KAFKA_TOPIC='orders_s', VALUE_FORMAT='JSON', PARTITIONS=2)",
        )
        .await;
        for (id, amount, status) in [
            ("1", 100, "new"),
            ("2", 200, "new"),
            ("3", 400, "processed"),
        ] {
            driver
                .query(
                    "INSERT INTO ORDERS_S (ID, AMOUNT, STATUS) VALUES (?, ?, ?)",
                    &[json!(id), json!(amount), json!(status)],
                    &QueryOptions::default(),
                )
                .await
                .unwrap();
        }
    })
    .await;
}

/// Builds `stb_pre_aggregations.<name>` like the orchestrator does.
async fn pre_aggregation(driver: &KsqlDriver, name: &str, select: &str) -> String {
    let table = format!("stb_pre_aggregations.{name}");
    let _ = driver.drop_table(&table, &QueryOptions::default()).await;
    driver
        .load_pre_aggregation_into_table(
            &table,
            &format!("CREATE TABLE `{table}` WITH (KEY_FORMAT='JSON') AS {select}"),
            &[],
            &offset("earliest"),
        )
        .await
        .unwrap();
    table
}

#[tokio::test]
async fn test_connection_and_select_guard() {
    let (url, _serial) = require_ksql!();
    let driver = driver(&url);
    driver.test_connection().await.unwrap();

    let err = driver
        .query("select * from ORDERS_S", &[], &QueryOptions::default())
        .await
        .unwrap_err();
    assert!(err
        .to_string()
        .starts_with("Select queries for ksql allowed only from Cube Store"));

    let err = driver
        .query("SHOW NONSENSE", &[], &QueryOptions::default())
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .starts_with("ksql API error for 'SHOW NONSENSE;': "),
        "{err}"
    );
}

#[tokio::test]
async fn kafka_connection_check() {
    let (url, _serial) = require_ksql!();
    let Some(kafka) = env("CUBEJS_TEST_KSQL_KAFKA_HOST") else {
        eprintln!("CUBEJS_TEST_KSQL_KAFKA_HOST is not set, skipping");
        return;
    };
    let mut c = config(&url);
    c.kafka_host = Some(format!(" {kafka} "));
    KsqlDriver::new(c).unwrap().test_connection().await.unwrap();

    let mut c = config(&url);
    c.kafka_host = Some("127.0.0.1:1".to_string());
    c.driver.test_connection_timeout = Duration::from_secs(3);
    let err = KsqlDriver::new(c)
        .unwrap()
        .test_connection()
        .await
        .unwrap_err();
    assert!(err.to_string().contains("kafka 127.0.0.1:1"), "{err}");
}

#[tokio::test]
async fn introspection_and_pre_aggregation_lifecycle() {
    let (url, _serial) = require_ksql!();
    let driver = driver(&url);
    orders_stream(&driver).await;
    let table = pre_aggregation(
        &driver,
        "orders_life",
        "SELECT STATUS, COUNT(*) AS CNT, SUM(AMOUNT) AS TOTAL FROM ORDERS_S GROUP BY STATUS",
    )
    .await;

    let tables = driver
        .get_tables_query("stb_pre_aggregations")
        .await
        .unwrap();
    assert!(tables.contains(&"orders_life".to_string()), "{tables:?}");
    // Case-insensitive schema match, like the Node driver.
    let tables = driver
        .get_tables_query("STB_PRE_AGGREGATIONS")
        .await
        .unwrap();
    assert!(tables.contains(&"orders_life".to_string()), "{tables:?}");

    let schema = driver.tables_schema().await.unwrap();
    let orders = &schema[""]["ORDERS_S"];
    let columns: Vec<(&str, &str)> = orders
        .iter()
        .map(|c| (c.name.as_str(), c.type_.as_str()))
        .collect();
    assert_eq!(
        columns,
        vec![("ID", "STRING"), ("AMOUNT", "DOUBLE"), ("STATUS", "STRING")]
    );
    assert!(schema["stb_pre_aggregations"].contains_key("orders_life"));

    assert_eq!(
        driver.table_column_types(&table).await.unwrap(),
        vec![
            Column::new("STATUS", "text"),
            Column::new("CNT", "bigint"),
            Column::new("TOTAL", "DOUBLE"),
        ]
    );

    let data = driver
        .download_table_streaming(&table, Some("earliest".into()))
        .await
        .unwrap();
    assert_eq!(data.streaming_table, "stb_pre_aggregations-orders_life");
    assert_eq!(data.stream_offset.as_deref(), Some("earliest"));
    assert_eq!(data.partitions, Some(2));
    assert_eq!(data.types.len(), 3);
    assert_eq!(
        data.streaming_source,
        StreamingSource {
            name: "default".into(),
            type_: "ksql".into(),
            credentials: vec![
                ("user".into(), Value::Null),
                ("password".into(), Value::Null),
                ("url".into(), json!(url)),
            ],
        }
    );

    driver
        .drop_table(&table, &QueryOptions::default())
        .await
        .unwrap();
    let tables = driver
        .get_tables_query("stb_pre_aggregations")
        .await
        .unwrap();
    assert!(!tables.contains(&"orders_life".to_string()), "{tables:?}");
}

#[tokio::test]
async fn windowed_tables_expose_window_bounds() {
    let (url, _serial) = require_ksql!();
    let driver = driver(&url);
    orders_stream(&driver).await;
    let table = pre_aggregation(
        &driver,
        "orders_windowed",
        "SELECT STATUS, COUNT(*) AS CNT FROM ORDERS_S WINDOW TUMBLING (SIZE 1 HOUR) GROUP BY STATUS",
    )
    .await;
    assert_eq!(
        driver.table_column_types(&table).await.unwrap(),
        vec![
            Column::new("STATUS", "text"),
            Column::new("WINDOWSTART", "int"),
            Column::new("WINDOWEND", "int"),
            Column::new("CNT", "bigint"),
        ]
    );
    driver
        .drop_table(&table, &QueryOptions::default())
        .await
        .unwrap();
}

#[tokio::test]
async fn download_query_results_describes_the_source() {
    let (url, _serial) = require_ksql!();
    let driver = driver(&url);
    orders_stream(&driver).await;
    let data = driver
        .download_query_results_streaming(
            "SELECT * FROM ORDERS_S WHERE STATUS = ?",
            &[json!("it's")],
            Some("latest".into()),
            Some(vec![
                Column::new("ID", "text"),
                Column::new("AMOUNT", "double"),
            ]),
        )
        .await
        .unwrap();
    assert_eq!(data.streaming_table, "ORDERS_S");
    assert_eq!(
        data.select_statement.as_deref(),
        Some("SELECT * FROM ORDERS_S WHERE STATUS = 'it''s'")
    );
    assert_eq!(
        data.types,
        vec![Column::new("ID", "text"), Column::new("AMOUNT", "double")]
    );
    let source = data.source_table.unwrap();
    assert_eq!(source.table_name, "ORDERS_S");
    assert_eq!(
        source.types,
        vec![
            Column::new("ID", "text"),
            Column::new("AMOUNT", "DOUBLE"),
            Column::new("STATUS", "text"),
        ]
    );

    // Direct Kafka download: the source becomes the stream's topic.
    let mut c = config(&url);
    c.kafka_host = Some("broker:9092".into());
    let data = KsqlDriver::new(c)
        .unwrap()
        .download_query_results_streaming("SELECT * FROM ORDERS_S", &[], None, None)
        .await
        .unwrap();
    assert_eq!(data.streaming_table, "orders_s");
    assert_eq!(data.streaming_source.name, "default-kafka");
    assert_eq!(
        data.locations(),
        vec![
            "stream://default-kafka/orders_s/0",
            "stream://default-kafka/orders_s/1"
        ]
    );
}

/// The whole streaming pre-aggregation: ksqlDB table → Cube Store streaming
/// table through `CREATE SOURCE` + `LOCATION 'stream://…'`.
#[tokio::test]
async fn streaming_import_into_cube_store() {
    let (url, _serial) = require_ksql!();
    let (Some(cubestore_url), Some(url_for_cubestore)) = (
        env("CUBEJS_TEST_KSQL_CUBESTORE_URL"),
        env("CUBEJS_TEST_KSQL_URL_FOR_CUBESTORE"),
    ) else {
        eprintln!("CUBEJS_TEST_KSQL_CUBESTORE_URL / _URL_FOR_CUBESTORE are not set, skipping");
        return;
    };
    let driver = driver(&url);
    orders_stream(&driver).await;
    let table = pre_aggregation(
        &driver,
        "orders_cs",
        "SELECT STATUS, COUNT(*) AS CNT, SUM(AMOUNT) AS TOTAL FROM ORDERS_S GROUP BY STATUS",
    )
    .await;
    let mut data = driver
        .download_table_streaming(&table, Some("earliest".into()))
        .await
        .unwrap();
    // Cube Store reaches ksqlDB by its container name.
    data.streaming_source.credentials[2].1 = json!(url_for_cubestore);

    let cubestore = CubeStoreDriver::new(CubeStoreConfig::from_url(&cubestore_url)).unwrap();
    cubestore
        .create_schema_if_not_exists("stb_pre_aggregations")
        .await
        .unwrap();
    let target = "stb_pre_aggregations.orders_cs_target";
    let _ = cubestore.drop_table(target, &QueryOptions::default()).await;
    data.import_into_cube_store(
        &cubestore,
        target,
        &data.types,
        Some(&["STATUS".to_string()]),
        &ExternalCreateTableOptions::default(),
        &QueryOptions::default(),
    )
    .await
    .unwrap();

    let mut rows = Vec::new();
    for _ in 0..60 {
        rows = cubestore
            .query(
                &format!("SELECT STATUS, CNT, TOTAL FROM {target} ORDER BY STATUS"),
                &[],
                &QueryOptions::default(),
            )
            .await
            .unwrap()
            .rows;
        if rows.len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0][0], json!("new"));
    assert_eq!(rows[1][0], json!("processed"));

    let err = data
        .import_into_cube_store(
            &cubestore,
            target,
            &data.types,
            None,
            &ExternalCreateTableOptions::default(),
            &QueryOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().starts_with("Older version of orchestrator"));

    cubestore
        .drop_table(target, &QueryOptions::default())
        .await
        .unwrap();
    driver
        .drop_table(&table, &QueryOptions::default())
        .await
        .unwrap();
}

/// Direct Kafka download: Cube Store consumes the ksqlDB source stream's
/// topic itself (`type: 'kafka'`), filtered by the `select_statement`.
#[tokio::test]
async fn kafka_streaming_import_into_cube_store() {
    let (url, _serial) = require_ksql!();
    let (Some(cubestore_url), Some(kafka_for_cubestore)) = (
        env("CUBEJS_TEST_KSQL_CUBESTORE_URL"),
        env("CUBEJS_TEST_KSQL_KAFKA_FOR_CUBESTORE"),
    ) else {
        eprintln!("CUBEJS_TEST_KSQL_CUBESTORE_URL / _KAFKA_FOR_CUBESTORE are not set, skipping");
        return;
    };
    // Cube Store resolves the `select_statement`'s table against the source
    // table, which is the topic here: the stream is named after its topic
    // (the same constraint applies to the Node driver).
    let plain = driver(&url);
    exec(
        &plain,
        "CREATE STREAM IF NOT EXISTS ORDERS_K (ID STRING KEY, AMOUNT DOUBLE, STATUS STRING) \
         WITH (KAFKA_TOPIC='ORDERS_K', VALUE_FORMAT='JSON', PARTITIONS=2)",
    )
    .await;
    for (id, amount, status) in [
        ("1", 100, "new"),
        ("2", 200, "new"),
        ("3", 400, "processed"),
    ] {
        plain
            .query(
                "INSERT INTO ORDERS_K (ID, AMOUNT, STATUS) VALUES (?, ?, ?)",
                &[json!(id), json!(amount), json!(status)],
                &QueryOptions::default(),
            )
            .await
            .unwrap();
    }
    let mut c = config(&url);
    c.kafka_host = Some(kafka_for_cubestore);
    c.streaming_source_name = Some("drvtest".into());
    let driver = KsqlDriver::new(c).unwrap();
    let data = driver
        .download_query_results_streaming(
            "SELECT * FROM ORDERS_K",
            &[],
            Some("earliest".into()),
            Some(vec![
                Column::new("ID", "text"),
                Column::new("AMOUNT", "double"),
                Column::new("STATUS", "text"),
            ]),
        )
        .await
        .unwrap();

    let cubestore = CubeStoreDriver::new(CubeStoreConfig::from_url(&cubestore_url)).unwrap();
    cubestore
        .create_schema_if_not_exists("stb_pre_aggregations")
        .await
        .unwrap();
    let target = "stb_pre_aggregations.orders_kafka_target";
    let _ = cubestore.drop_table(target, &QueryOptions::default()).await;
    data.import_into_cube_store(
        &cubestore,
        target,
        &data.types,
        Some(&["ID".to_string()]),
        &ExternalCreateTableOptions::default(),
        &QueryOptions::default(),
    )
    .await
    .unwrap();

    // Cube Store consumes the topic's JSON values. The stream's key lives in
    // the Kafka record key, not in the value, so `ID` arrives as NULL and the
    // unique key collapses the rows: what matters here is that the rows come
    // through the `kafka` source at all.
    let mut rows = Vec::new();
    for _ in 0..60 {
        rows = cubestore
            .query(
                &format!("SELECT AMOUNT, STATUS FROM {target}"),
                &[],
                &QueryOptions::default(),
            )
            .await
            .unwrap()
            .rows;
        if !rows.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(!rows.is_empty(), "no rows arrived from Kafka");
    for row in &rows {
        assert!(
            row[1] == json!("new") || row[1] == json!("processed"),
            "{rows:?}"
        );
    }
    cubestore
        .drop_table(target, &QueryOptions::default())
        .await
        .unwrap();
}
