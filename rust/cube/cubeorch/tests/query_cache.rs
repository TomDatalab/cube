//! `QueryCache` behaviour, ported from
//! `packages/cubejs-query-orchestrator/test/unit/QueryCache.abstract.ts`.

use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use async_trait::async_trait;
use cubecache::{CacheEntry, CacheKey};
use cubedriver::{
    Column, Driver, DriverConfig, DriverError, GenericType, QueryOptions, QueryResult,
};
use cubeorch::{
    cache::{CacheQueryResultOptions, LoadRefreshKeyOptions, RefreshKeyCacheOptions},
    types::RefreshKeyQueryOptions,
    DriverFactory, LocalRefreshKeyDescriptor, QueryCache, QueryCacheOptions, QueryWithParams,
};
use serde_json::{json, Value};

/// A driver that answers every statement with one row and counts the calls.
struct CountingDriver {
    config: DriverConfig,
    executed: Mutex<Vec<String>>,
    calls: AtomicUsize,
}

impl CountingDriver {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            config: DriverConfig::default(),
            executed: Mutex::new(Vec::new()),
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn executed(&self) -> Vec<String> {
        self.executed.lock().unwrap().clone()
    }
}

#[async_trait]
impl Driver for CountingDriver {
    fn config(&self) -> &DriverConfig {
        &self.config
    }

    async fn test_connection(&self) -> Result<(), DriverError> {
        Ok(())
    }

    async fn query(
        &self,
        sql: &str,
        _params: &[Value],
        _options: &QueryOptions,
    ) -> Result<QueryResult, DriverError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.executed.lock().unwrap().push(sql.to_string());

        Ok(QueryResult::new(
            vec![Column::new("value", GenericType::String)],
            vec![vec![json!("new-result")]],
        ))
    }
}

fn cache_with(
    driver: Arc<CountingDriver>,
    options: QueryCacheOptions,
) -> (Arc<QueryCache>, Arc<Mutex<Vec<String>>>) {
    let messages = Arc::new(Mutex::new(Vec::new()));
    let sink = messages.clone();

    let factory: DriverFactory = Arc::new(move |_| {
        let driver = driver.clone();

        Box::pin(async move { Ok(driver as Arc<dyn Driver>) })
    });

    let cache = QueryCache::builder("test", factory)
        .logger(Arc::new(move |message, _| {
            sink.lock().unwrap().push(message.to_string());
        }))
        .options(options)
        .build();

    (cache, messages)
}

fn cache(driver: Arc<CountingDriver>) -> (Arc<QueryCache>, Arc<Mutex<Vec<String>>>) {
    cache_with(driver, QueryCacheOptions::default())
}

fn key(query: &str) -> CacheKey {
    QueryCache::query_body_cache_key(&cubeorch::QueryBody {
        query: Some(query.to_string()),
        values: Some(vec![]),
        ..Default::default()
    })
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// Seeds an entry whose `renewalKey` is hashed the way `fetchAndCacheQuery` stores it.
async fn seed(
    cache: &Arc<QueryCache>,
    cache_key: &CacheKey,
    age_secs: i64,
    renewal_key: Option<&CacheKey>,
    request_id: Option<&str>,
) {
    let entry = CacheEntry {
        time: now_ms() - age_secs * 1000,
        result: json!("cached-data"),
        renewal_key: renewal_key.map(|key| cache.query_cache_key(key)),
        request_id: request_id.map(str::to_string),
    };

    cubecache::set_entry(
        cache.cache_driver().as_ref(),
        &cache.query_cache_key(cache_key),
        &entry,
        3600,
    )
    .await
    .unwrap();
}

fn options(
    renewal_key: Option<CacheKey>,
    wait_for_renew: bool,
    request_id: &str,
    renew_cycle: bool,
) -> CacheQueryResultOptions {
    CacheQueryResultOptions {
        renewal_threshold: Some(600),
        renewal_key,
        wait_for_renew,
        request_id: Some(request_id.to_string()),
        renew_cycle,
        ..Default::default()
    }
}

/// Gives a detached background refresh a chance to run before the assertions.
async fn settle() {
    for _ in 0..50 {
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

fn new_result() -> Value {
    json!([{ "value": "new-result" }])
}

#[tokio::test]
async fn expired_and_wait_for_renew_blocks_on_the_fetch() {
    let driver = CountingDriver::new();
    let (cache, messages) = cache(driver.clone());
    let cache_key = key("expired-wait");
    let renewal_key = key("key-a");

    seed(&cache, &cache_key, 700, Some(&renewal_key), None).await;

    let result = cache
        .cache_query_result(
            "SELECT 1",
            &[],
            &cache_key,
            3600,
            options(Some(renewal_key), true, "req-1", false),
        )
        .await
        .unwrap();

    assert_eq!(result, new_result());
    assert_eq!(driver.calls(), 1);
    assert!(messages
        .lock()
        .unwrap()
        .contains(&"Waiting for renew".to_string()));
}

#[tokio::test]
async fn expired_without_wait_for_renew_serves_the_cache_and_refreshes_behind_it() {
    let driver = CountingDriver::new();
    let (cache, messages) = cache(driver.clone());
    let cache_key = key("expired-no-wait");
    let renewal_key = key("key-a");

    seed(&cache, &cache_key, 700, Some(&renewal_key), None).await;

    let result = cache
        .cache_query_result(
            "SELECT 1",
            &[],
            &cache_key,
            3600,
            options(Some(renewal_key), false, "req-2", false),
        )
        .await
        .unwrap();

    assert_eq!(result, json!("cached-data"));
    settle().await;
    assert_eq!(driver.calls(), 1);
    assert!(messages
        .lock()
        .unwrap()
        .contains(&"Renewing existing key".to_string()));
}

#[tokio::test]
async fn a_key_mismatch_while_not_expired_blocks_when_waiting_for_renew() {
    let driver = CountingDriver::new();
    let (cache, _) = cache(driver.clone());
    let cache_key = key("key-mismatch-user");

    seed(&cache, &cache_key, 100, Some(&key("key-old")), None).await;

    let result = cache
        .cache_query_result(
            "SELECT 1",
            &[],
            &cache_key,
            3600,
            options(Some(key("key-new")), true, "req-3", false),
        )
        .await
        .unwrap();

    assert_eq!(result, new_result());
    assert_eq!(driver.calls(), 1);
}

#[tokio::test]
async fn a_key_mismatch_in_a_renew_cycle_blocks_on_the_fetch() {
    let driver = CountingDriver::new();
    let (cache, _) = cache(driver.clone());
    let cache_key = key("key-mismatch-cycle");

    seed(
        &cache,
        &cache_key,
        100,
        Some(&key("key-old")),
        Some("req-4-span-1"),
    )
    .await;

    // The same request, but a renew cycle never serves what it is meant to replace.
    let result = cache
        .cache_query_result(
            "SELECT 1",
            &[],
            &cache_key,
            3600,
            options(Some(key("key-new")), true, "req-4-span-2", true),
        )
        .await
        .unwrap();

    assert_eq!(result, new_result());
}

#[tokio::test]
async fn the_same_request_gets_the_stale_result_and_a_background_refresh() {
    let driver = CountingDriver::new();
    let (cache, messages) = cache(driver.clone());
    let cache_key = key("same-request-expired");
    let renewal_key = key("key-a");

    seed(
        &cache,
        &cache_key,
        700,
        Some(&renewal_key),
        Some("req-5-span-1"),
    )
    .await;

    // The client polls through continue wait with the same request uuid. Rejecting the result
    // it just wrote would restart the fetch on every poll and never converge.
    let result = cache
        .cache_query_result(
            "SELECT 1",
            &[],
            &cache_key,
            3600,
            options(Some(renewal_key), true, "req-5-span-7", false),
        )
        .await
        .unwrap();

    assert_eq!(result, json!("cached-data"));
    settle().await;
    assert_eq!(driver.calls(), 1);
    assert!(messages
        .lock()
        .unwrap()
        .contains(&"Same request cache hit (background refresh)".to_string()));
}

#[tokio::test]
async fn a_matching_key_that_is_not_expired_never_fetches() {
    let driver = CountingDriver::new();
    let (cache, messages) = cache(driver.clone());
    let cache_key = key("fresh");
    let renewal_key = key("key-a");

    seed(&cache, &cache_key, 100, Some(&renewal_key), None).await;

    let result = cache
        .cache_query_result(
            "SELECT 1",
            &[],
            &cache_key,
            3600,
            options(Some(renewal_key), true, "req-6", false),
        )
        .await
        .unwrap();

    assert_eq!(result, json!("cached-data"));
    settle().await;
    assert_eq!(driver.calls(), 0);
    assert!(messages
        .lock()
        .unwrap()
        .contains(&"Using cache for".to_string()));
}

#[tokio::test]
async fn an_expired_entry_without_a_renewal_key_keeps_serving_the_cache() {
    let driver = CountingDriver::new();
    let (cache, _) = cache(driver.clone());
    let cache_key = key("no-renewal-key");

    seed(&cache, &cache_key, 700, None, None).await;

    let result = cache
        .cache_query_result(
            "SELECT 1",
            &[],
            &cache_key,
            3600,
            options(None, true, "req-7", false),
        )
        .await
        .unwrap();

    // Without a refresh key there is nothing to refresh against.
    assert_eq!(result, json!("cached-data"));
    settle().await;
    assert_eq!(driver.calls(), 0);
}

#[tokio::test]
async fn force_no_cache_always_fetches() {
    let driver = CountingDriver::new();
    let (cache, messages) = cache(driver.clone());
    let cache_key = key("force-no-cache");

    seed(&cache, &cache_key, 1, Some(&key("key-a")), None).await;

    let result = cache
        .cache_query_result(
            "SELECT 1",
            &[],
            &cache_key,
            3600,
            CacheQueryResultOptions {
                force_no_cache: true,
                ..options(Some(key("key-a")), false, "req-8", false)
            },
        )
        .await
        .unwrap();

    assert_eq!(result, new_result());
    assert!(messages
        .lock()
        .unwrap()
        .contains(&"Force no cache for".to_string()));
}

#[tokio::test]
async fn a_failed_fetch_drops_the_cache_entry() {
    struct FailingDriver(DriverConfig);

    #[async_trait]
    impl Driver for FailingDriver {
        fn config(&self) -> &DriverConfig {
            &self.0
        }

        async fn test_connection(&self) -> Result<(), DriverError> {
            Ok(())
        }

        async fn query(
            &self,
            _sql: &str,
            _params: &[Value],
            _options: &QueryOptions,
        ) -> Result<QueryResult, DriverError> {
            Err(DriverError::Query("boom".to_string()))
        }
    }

    let factory: DriverFactory = Arc::new(|_| {
        Box::pin(async { Ok(Arc::new(FailingDriver(DriverConfig::default())) as Arc<dyn Driver>) })
    });
    let cache = QueryCache::builder("test", factory).build();
    let cache_key = key("failing");

    seed(&cache, &cache_key, 700, Some(&key("key-a")), None).await;

    let error = cache
        .cache_query_result(
            "SELECT 1",
            &[],
            &cache_key,
            3600,
            options(Some(key("key-a")), true, "req-9", false),
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("boom"));
    assert_eq!(
        cache
            .cache_driver()
            .get(&cache.query_cache_key(&cache_key))
            .await
            .unwrap(),
        None
    );
}

// ----------------------------------------------------------------------------
// Refresh keys
// ----------------------------------------------------------------------------

fn local_refresh_key_query(interval: f64) -> QueryWithParams {
    QueryWithParams::new(
        "SELECT FLOOR(EXTRACT(EPOCH FROM NOW())) as refresh_key",
        vec![],
    )
    .with_options(RefreshKeyQueryOptions {
        local_refresh_key: Some(LocalRefreshKeyDescriptor::new(interval, 0.0, 0.0)),
        ..Default::default()
    })
}

#[tokio::test]
async fn a_local_refresh_key_is_evaluated_without_touching_the_driver() {
    let driver = CountingDriver::new();
    let (cache, _) = cache_with(
        driver.clone(),
        QueryCacheOptions {
            local_refresh_key: true,
            ..Default::default()
        },
    );

    let result = cache
        .cache_refresh_key_result(
            &local_refresh_key_query(600.0),
            3600,
            &RefreshKeyCacheOptions {
                data_source: "default".to_string(),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(result.len(), 1);
    assert!(result[0]["refresh_key"].is_string());
    assert_eq!(driver.calls(), 0);
    assert!(cache.is_local_refresh_key_active());
}

#[tokio::test]
async fn a_local_refresh_key_still_runs_as_a_query_when_the_flag_is_off() {
    let driver = CountingDriver::new();
    let (cache, _) = cache(driver.clone());

    cache
        .cache_refresh_key_result(
            &local_refresh_key_query(600.0),
            3600,
            &RefreshKeyCacheOptions {
                data_source: "default".to_string(),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(driver.calls(), 1);
    assert!(!cache.is_local_refresh_key_active());
}

#[tokio::test]
async fn a_configured_renewal_threshold_keeps_local_keys_on_the_sql_path() {
    let driver = CountingDriver::new();
    let (cache, _) = cache_with(
        driver.clone(),
        QueryCacheOptions {
            local_refresh_key: true,
            // The override bounds how often the key advances, which a locally evaluated key
            // cannot honour, so it stays a query.
            refresh_key_renewal_threshold: Some(86400),
            ..Default::default()
        },
    );

    cache
        .cache_refresh_key_result(
            &local_refresh_key_query(600.0),
            3600,
            &RefreshKeyCacheOptions {
                data_source: "default".to_string(),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(driver.calls(), 1);
    assert!(!cache.is_local_refresh_key_active());
}

#[tokio::test]
async fn a_malformed_local_descriptor_stays_on_the_sql_path() {
    let driver = CountingDriver::new();
    let (cache, _) = cache_with(
        driver.clone(),
        QueryCacheOptions {
            local_refresh_key: true,
            ..Default::default()
        },
    );

    cache
        .cache_refresh_key_result(
            &local_refresh_key_query(0.0),
            3600,
            &RefreshKeyCacheOptions {
                data_source: "default".to_string(),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(driver.calls(), 1);
}

#[tokio::test]
async fn one_refresh_key_shared_by_several_queries_is_loaded_once() {
    let driver = CountingDriver::new();
    let (cache, _) = cache(driver.clone());

    let query = QueryWithParams::new("SELECT MAX(id) FROM t", vec![]);
    let values = cache
        .load_refresh_keys(
            &[query.clone(), query.clone(), query],
            3600,
            &LoadRefreshKeyOptions {
                data_source: "default".to_string(),
                request_id: Some("req-10".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(values.len(), 3);
    assert_eq!(values[0], values[1]);
    assert_eq!(values[1], values[2]);
    // The first load caches the result, so at most one query reaches the data source.
    assert_eq!(driver.calls(), 1);
}

#[tokio::test]
async fn refresh_keys_of_different_data_sources_do_not_share_an_entry() {
    let driver = CountingDriver::new();
    let (cache, _) = cache(driver.clone());
    let query = QueryWithParams::new("SELECT MAX(id) FROM t", vec![]);

    for data_source in ["default", "other"] {
        cache
            .cache_refresh_key_result(
                &query,
                3600,
                &RefreshKeyCacheOptions {
                    data_source: data_source.to_string(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }

    assert_eq!(driver.calls(), 2);
    assert_ne!(
        cache.refresh_key_cache_key(&query, "default"),
        cache.refresh_key_cache_key(&query, "other")
    );
}

#[tokio::test]
async fn the_query_cache_key_has_the_documented_shape() {
    let driver = CountingDriver::new();
    let (cache, _) = cache(driver);

    let cache_key = key("SELECT 1");

    assert!(cache
        .query_cache_key(&cache_key)
        .starts_with("test#SQL_QUERY_RESULT:"));
    assert_eq!(cache.get_key("CATALOG", "k"), "test#CATALOG:k");
}

#[tokio::test]
async fn a_cold_cached_query_runs_the_main_query_once() {
    let driver = CountingDriver::new();
    let (cache, _) = cache(driver.clone());

    let body = cubeorch::QueryBody {
        query: Some("SELECT 1".to_string()),
        values: Some(vec![]),
        data_source: Some("default".to_string()),
        cache_key_queries: Some(cubeorch::CacheKeyQueries::List(vec![QueryWithParams::new(
            "SELECT MAX(id) FROM t",
            vec![],
        )])),
        request_id: Some("req-11".to_string()),
        ..Default::default()
    };

    let first = cache
        .cached_query_result(&body, &[])
        .await
        .unwrap()
        .into_result()
        .expect("a query that is not persistent answers with rows");
    assert_eq!(first.data, new_result());
    assert_eq!(first.refresh_key_values.as_ref().map(Vec::len), Some(1));
    assert!(first.last_refresh_time.is_some());

    settle().await;

    // The main query and its refresh key, each once — the renew cycle that follows the
    // foreground renewal reuses both cache entries.
    assert_eq!(
        driver
            .executed()
            .iter()
            .filter(|s| *s == "SELECT 1")
            .count(),
        1
    );
}

#[tokio::test]
async fn pre_aggregation_table_names_are_replaced_in_the_query_and_in_its_refresh_keys() {
    let driver = CountingDriver::new();
    let (cache, _) = cache(driver.clone());

    let body = cubeorch::QueryBody {
        query: Some("SELECT * FROM pa.orders_rollup".to_string()),
        values: Some(vec![]),
        data_source: Some("default".to_string()),
        cache_key_queries: Some(cubeorch::CacheKeyQueries::List(vec![QueryWithParams::new(
            "SELECT MAX(x) FROM pa.orders_rollup",
            vec![],
        )])),
        ..Default::default()
    };

    let temp_table = cubeorch::LoadPreAggregationResult {
        target_table_name: "pa.orders_rollup_aaa_bbb_1fm6652".to_string(),
        ..Default::default()
    };

    cache
        .cached_query_result(&body, &[("pa.orders_rollup".to_string(), temp_table)])
        .await
        .unwrap();

    settle().await;

    let executed = driver.executed();
    assert!(executed
        .iter()
        .any(|sql| sql == "SELECT * FROM pa.orders_rollup_aaa_bbb_1fm6652"));
    assert!(executed
        .iter()
        .any(|sql| sql == "SELECT MAX(x) FROM pa.orders_rollup_aaa_bbb_1fm6652"));
    assert!(!executed
        .iter()
        .any(|sql| sql == "SELECT * FROM pa.orders_rollup"));
}
