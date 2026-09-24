#![cfg(feature = "pinot")]
//! Integration tests of the Pinot driver.
//!
//! They run only when `CUBEJS_TEST_PINOT_HOST` is set, against a broker with
//! the `baseballStats` table of the batch quick-start:
//!
//! ```text
//! docker run -d -p 127.0.0.1:16595:8000 apachepinot/pinot QuickStart -type batch
//! CUBEJS_TEST_PINOT_HOST=127.0.0.1 CUBEJS_TEST_PINOT_PORT=16595
//! ```

use cubedriver::pinot::{PinotConfig, PinotDriver};
use cubedriver::{
    Column, DownloadQueryResultsOptions, DownloadedData, Driver, DriverConfig, QueryOptions,
};
use serde_json::json;

fn server() -> Option<(String, u16)> {
    let host = std::env::var("CUBEJS_TEST_PINOT_HOST")
        .ok()
        .filter(|s| !s.is_empty())?;
    let port = std::env::var("CUBEJS_TEST_PINOT_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8099);
    Some((host, port))
}

macro_rules! require_server {
    () => {
        match server() {
            Some(s) => s,
            None => {
                eprintln!("CUBEJS_TEST_PINOT_HOST is not set, skipping");
                return;
            }
        }
    };
}

fn driver(host: &str, port: u16, null_handling: Option<bool>) -> PinotDriver {
    let mut config = PinotConfig::from_driver_config(DriverConfig::default());
    config.host = host.to_string();
    config.port = Some(port);
    config.null_handling = null_handling;
    PinotDriver::new(config).unwrap()
}

#[tokio::test]
async fn test_connection() {
    let (host, port) = require_server!();
    driver(&host, port, None).test_connection().await.unwrap();
    assert!(driver(&host, 1, None).test_connection().await.is_err());
}

#[tokio::test]
async fn query_with_params_and_types() {
    let (host, port) = require_server!();
    for null_handling in [None, Some(true), Some(false)] {
        let driver = driver(&host, port, null_handling);
        let data = driver
            .query(
                "SELECT teamID, SUM(homeRuns) AS hr, AVG(CAST(hits AS DOUBLE)) AS avg_hits, COUNT(*) AS c
                 FROM baseballStats
                 WHERE teamID IN (?) AND yearID = ?
                 GROUP BY teamID
                 ORDER BY teamID
                 LIMIT 10",
                &[json!(["BOS", "NYA"]), json!(2000)],
                &QueryOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(data.len(), 2);
        assert_eq!(data.get(0, "teamID"), Some(&json!("BOS")));
        assert_eq!(data.get(1, "teamID"), Some(&json!("NYA")));
        assert_eq!(data.columns[0], Column::new("teamID", "text"));
        assert_eq!(data.columns[3], Column::new("c", "bigint"));
        assert!(data.get_i64(0, "c").unwrap() > 0);
    }

    // A quote in a parameter stays inside the literal.
    let data = driver(&host, port, None)
        .query(
            "SELECT COUNT(*) AS c FROM baseballStats WHERE playerName = ?",
            &[json!("O'Neill ' OR 1=1 --")],
            &QueryOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(data.get_i64(0, "c"), Some(0));
}

#[tokio::test]
async fn download_query_results_carries_the_broker_types() {
    let (host, port) = require_server!();
    let DownloadedData::Memory(data) = driver(&host, port, None)
        .download_query_results(
            "SELECT playerName, yearID, homeRuns FROM baseballStats ORDER BY yearID, playerName LIMIT 5",
            &[],
            &DownloadQueryResultsOptions::default(),
        )
        .await
        .unwrap()
    else {
        panic!("expected memory data");
    };
    assert_eq!(data.len(), 5);
    assert_eq!(
        data.columns,
        vec![
            Column::new("playerName", "text"),
            Column::new("yearID", "int"),
            Column::new("homeRuns", "int"),
        ]
    );
}

#[tokio::test]
async fn errors_carry_the_broker_message() {
    let (host, port) = require_server!();
    let err = driver(&host, port, None)
        .query("SELECT * FROM no_such_table", &[], &QueryOptions::default())
        .await
        .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("no_such_table"),
        "unexpected error: {err}"
    );
}
