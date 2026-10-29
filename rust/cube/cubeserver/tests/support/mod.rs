//! Helpers shared by the integration tests that need Postgres.
//!
//! Each test works in its own throw-away database on the server named by
//! `CUBE_TEST_PG_URL`; without it the tests skip.
#![allow(dead_code)]

use cubeserver::rbac::store::Store;
use tokio_postgres::NoTls;
use uuid::Uuid;

pub struct TestDb {
    pub admin_url: String,
    pub name: String,
    pub store: Store,
}

impl TestDb {
    pub async fn create() -> Option<Self> {
        let Ok(admin_url) = std::env::var("CUBE_TEST_PG_URL") else {
            eprintln!("CUBE_TEST_PG_URL not set; skipping");
            return None;
        };
        let name = format!("cube_app_test_{}", Uuid::new_v4().simple());
        let admin = connect(&admin_url).await;
        admin
            .batch_execute(&format!("CREATE DATABASE {name}"))
            .await
            .unwrap();
        let store = Store::connect_with_pool_size(&with_db(&admin_url, &name), 1)
            .await
            .unwrap();
        Some(Self {
            admin_url,
            name,
            store,
        })
    }

    pub async fn drop_db(self) {
        let Self {
            admin_url,
            name,
            store,
        } = self;
        drop(store);
        let admin = connect(&admin_url).await;
        admin
            .batch_execute(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"))
            .await
            .unwrap();
    }
}

/// `url` pointed at database `db`, keeping any query string.
pub fn with_db(url: &str, db: &str) -> String {
    let (main, query) = match url.split_once('?') {
        Some((m, q)) => (m, Some(q)),
        None => (url, None),
    };
    let (base, _) = main.rsplit_once('/').unwrap();
    match query {
        Some(q) => format!("{base}/{db}?{q}"),
        None => format!("{base}/{db}"),
    }
}

pub async fn connect(url: &str) -> tokio_postgres::Client {
    let (client, conn) = tokio_postgres::connect(url, NoTls).await.unwrap();
    tokio::spawn(conn);
    client
}
