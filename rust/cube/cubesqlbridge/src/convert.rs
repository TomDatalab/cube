//! SQL → Cube query conversion, the Rust port of
//! `packages/cubejs-backend-native/src/rest4sql.rs` and `sql4sql.rs` without
//! the Neon layer.
//!
//! Both serve API endpoints: `/v1/convert-query` turns a SQL statement into
//! the REST query it is equivalent to, and `/v1/cubesql` reports the plan a
//! statement produces.

use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;

use cubesql::compile::datafusion::logical_plan::LogicalPlan;
use cubesql::compile::engine::df::scan::CubeScanNode;
use cubesql::compile::{convert_sql_to_cube_query, DatabaseProtocol};
use cubesql::config::{ConfigObj, CubeServices};
use cubesql::sql::{Session, SessionManager};
use cubesql::transport::TransportLoadRequestQuery;
use cubesql::CubeError;
use serde::Serialize;
use serde_json::Value;

use crate::auth::RustSqlAuthContext;

/// The `/v1/convert-query` answer (`Rest4SqlResponse` in the Neon module).
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum ConvertedQuery {
    Ok {
        status: &'static str,
        query: Box<TransportLoadRequestQuery>,
    },
    Error {
        status: &'static str,
        error: String,
    },
}

impl ConvertedQuery {
    fn error(message: impl Into<String>) -> Self {
        ConvertedQuery::Error {
            status: "error",
            error: message.into(),
        }
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, ConvertedQuery::Ok { .. })
    }
}

/// Opens a session, runs `f`, and closes the session even when `f` fails.
async fn with_session<T, F, Fut>(
    services: &CubeServices,
    auth_context: Arc<RustSqlAuthContext>,
    f: F,
) -> Result<T, CubeError>
where
    F: FnOnce(Arc<Session>) -> Fut,
    Fut: std::future::Future<Output = Result<T, CubeError>>,
{
    let config = services.injector.get_service_typed::<dyn ConfigObj>().await;
    let session_manager = services
        .injector
        .get_service_typed::<SessionManager>()
        .await;

    let (host, port) = match SocketAddr::from_str(
        config
            .postgres_bind_address()
            .as_deref()
            .unwrap_or("127.0.0.1:15432"),
    ) {
        Ok(addr) => (addr.ip().to_string(), addr.port()),
        Err(e) => {
            return Err(CubeError::internal(format!(
                "Failed to parse postgres_bind_address: {e}"
            )))
        }
    };

    let session = session_manager
        .create_session(DatabaseProtocol::PostgreSQL, host, port, None)
        .await?;
    session.state.set_auth_context(Some(auth_context));
    let connection_id = session.state.connection_id;

    let result = f(session).await;

    session_manager.drop_session(connection_id).await;

    result
}

fn auth_context(security_context: Option<Value>) -> Arc<RustSqlAuthContext> {
    Arc::new(RustSqlAuthContext {
        user: None,
        superuser: false,
        security_context,
    })
}

/// `POST /v1/convert-query`: the REST query a SQL statement is equivalent to.
///
/// A statement that does not reduce to a single cube scan is reported as an
/// error in the body, not as a transport failure, exactly as the Node.js
/// endpoint does.
pub async fn rest4sql(
    services: &CubeServices,
    sql_query: &str,
    security_context: Option<Value>,
) -> Result<ConvertedQuery, CubeError> {
    let auth = auth_context(security_context);

    with_session(services, auth.clone(), |session| async move {
        let transport = session.server.transport.clone();
        let meta = transport.meta(auth).await?;

        let query_plan = convert_sql_to_cube_query(sql_query, meta, session).await?;

        let LogicalPlan::Extension(extension) = query_plan.try_as_logical_plan()? else {
            return Ok(ConvertedQuery::error(
                "Provided sql query can not be converted to rest query.",
            ));
        };

        match extension.node.as_any().downcast_ref::<CubeScanNode>() {
            Some(cube_scan) => Ok(ConvertedQuery::Ok {
                status: "ok",
                query: Box::new(cube_scan.request.clone()),
            }),
            None => Ok(ConvertedQuery::error(
                "Provided sql query can not be converted to rest query.",
            )),
        }
    })
    .await
}

/// `POST /v1/cubesql`: how a statement is answered — by a cube scan that the
/// data source runs, or by post-processing on top of one.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Sql4SqlPlan {
    /// The statement maps onto one cube scan.
    Ok {
        /// The SQL sent to the data source.
        sql: String,
        /// Its bound parameters.
        values: Vec<Option<String>>,
    },
    /// The statement needs post-processing that the data source cannot do.
    PostProcessing {
        query: Box<TransportLoadRequestQuery>,
    },
    Error {
        error: String,
    },
}

/// Plans `sql_query` and reports how it would be answered.
pub async fn sql4sql(
    services: &CubeServices,
    sql_query: &str,
    security_context: Option<Value>,
) -> Result<Sql4SqlPlan, CubeError> {
    let auth = auth_context(security_context);

    with_session(services, auth.clone(), |session| async move {
        let transport = session.server.transport.clone();
        let meta = transport.meta(auth.clone()).await?;

        let query_plan = match convert_sql_to_cube_query(sql_query, meta, session.clone()).await {
            Ok(plan) => plan,
            Err(err) => {
                return Ok(Sql4SqlPlan::Error {
                    error: err.to_string(),
                })
            }
        };

        let LogicalPlan::Extension(extension) = query_plan.try_as_logical_plan()? else {
            return Ok(Sql4SqlPlan::Error {
                error: "Provided sql query can not be planned.".to_string(),
            });
        };

        let Some(cube_scan) = extension.node.as_any().downcast_ref::<CubeScanNode>() else {
            return Ok(Sql4SqlPlan::Error {
                error: "Provided sql query can not be planned.".to_string(),
            });
        };

        // A scan the data source can run directly is reported with its SQL;
        // anything else is answered by post-processing over the scan.
        // The scan node carries the span and auth the planner attached.
        let response = transport
            .sql(
                cube_scan.span_id.clone(),
                cube_scan.request.clone(),
                cube_scan.auth_context.clone(),
                session.state.get_load_request_meta("sql"),
                None,
                None,
            )
            .await;

        Ok(match response {
            Ok(sql) => Sql4SqlPlan::Ok {
                sql: sql.sql.sql,
                values: sql.sql.values,
            },
            Err(_) => Sql4SqlPlan::PostProcessing {
                query: Box::new(cube_scan.request.clone()),
            },
        })
    })
    .await
}
