//! [`RustTransport`]: the SQL API's [`TransportService`], with no JavaScript.
//!
//! It replaces `NodeBridgeTransport` method for method. `meta`, `compiler_id`,
//! `sql`, `can_switch_user_for_session` and `log_load_state` are answered from
//! the compiled model, the planner and the process's own configuration.
//! `load` and `load_stream` are the two that genuinely need the query
//! orchestrator, so they go to an injected [`QueryExecutor`].

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use cubesql::compile::engine::df::scan::{
    convert_transport_response, CacheMode, MemberField, RecordBatch, SchemaRef,
};
use cubesql::compile::engine::df::wrapper::SqlQuery;
use cubesql::di_service;
use cubesql::sql::{AuthContextRef, SqlAuthService, SqlAuthServiceAuthenticateRequest};
use cubesql::transport::TransportLoadRequestQuery;
use cubesql::transport::{
    CubeStreamReceiver, LoadRequestMeta, MetaContext, SpanId, SqlResponse, TransportService,
};
use cubesql::CubeError;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::auth::{RustSqlAuthContext, SqlAuthConfig};
use crate::executor::QueryExecutor;
use crate::meta::meta_context;
use crate::model_source::ModelSource;
use crate::planner::{plan_options, to_planner_query, DialectMap, PlannerPool};

/// The session fields every request to the transport carries.
#[derive(Debug, Clone, Default)]
struct Session {
    user: Option<String>,
    superuser: bool,
    security_context: Option<Value>,
}

impl Session {
    fn from_context(ctx: &AuthContextRef) -> Self {
        match ctx.as_any().downcast_ref::<RustSqlAuthContext>() {
            Some(native) => Self {
                user: native.user.clone(),
                superuser: native.superuser,
                security_context: native.security_context.clone(),
            },
            // Another `AuthContext` implementation is still usable: the parts
            // the transport needs are on the trait itself. Only `superuser`
            // has no trait accessor, and defaulting it to `false` is the safe
            // direction.
            None => Self {
                user: ctx.user().cloned(),
                superuser: false,
                security_context: ctx.security_context().cloned(),
            },
        }
    }

    fn to_json(&self) -> Value {
        let mut session = Map::new();
        session.insert(
            "user".to_string(),
            self.user.clone().map(Value::String).unwrap_or(Value::Null),
        );
        session.insert("superuser".to_string(), Value::Bool(self.superuser));
        if let Some(security_context) = &self.security_context {
            session.insert("securityContext".to_string(), security_context.clone());
        }
        Value::Object(session)
    }

    fn security_context_value(&self) -> Value {
        self.security_context
            .clone()
            .unwrap_or(Value::Object(Map::new()))
    }
}

/// How long a continue wait is left alone before the request is sent again.
///
/// An executor is expected to block until it has an answer, so a continue wait
/// normally comes back after seconds, not microseconds. This only keeps a
/// misbehaving executor that answers instantly from spinning a core.
const CONTINUE_WAIT_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(50);

/// `${requestId}-span-N`, the request id shape the log sink and query history
/// expect. An id that already names a span is left alone.
fn span_request_id(span_id: Option<&Arc<SpanId>>, sequence: u32) -> String {
    let request_id = span_id
        .map(|s| s.span_id.clone())
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    if request_id.contains("-span-") {
        request_id
    } else {
        format!("{request_id}-span-{sequence}")
    }
}

/// How the transport is put together.
pub struct RustTransportOptions {
    /// Where the data model is read from.
    pub model_source: ModelSource,
    /// How many planner threads to run. Each owns its own compiled model.
    pub planner_threads: usize,
    /// The dialect each data source renders in.
    pub dialects: DialectMap,
    /// Keep members that are not public in the meta the SQL API exposes. This
    /// is the dev-mode branch of `filterVisibleItemsInMeta`.
    pub include_hidden_members: bool,
    /// Runs the queries `load` / `load_stream` plan.
    pub executor: Arc<dyn QueryExecutor>,
    /// `CUBEJS_SQL_USER` / `CUBEJS_SQL_SUPER_USER`, for user switching.
    pub auth_config: SqlAuthConfig,
    /// Re-authenticates a `__user` switch so the new user gets its own
    /// security context, the way `contextByRequest` does in Node.
    pub auth_service: Option<Arc<dyn SqlAuthService>>,
}

/// A [`TransportService`] served entirely from Rust.
pub struct RustTransport {
    meta: Arc<MetaContext>,
    planner: PlannerPool,
    dialects: DialectMap,
    executor: Arc<dyn QueryExecutor>,
    auth_config: SqlAuthConfig,
    auth_service: Option<Arc<dyn SqlAuthService>>,
}

impl std::fmt::Debug for RustTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RustTransport")
            .field("compiler_id", &self.meta.compiler_id)
            .field("cubes", &self.meta.cubes.len())
            .field("planner", &self.planner)
            .field("executor", &self.executor)
            .finish()
    }
}

di_service!(RustTransport, [TransportService]);

impl RustTransport {
    /// Compiles the model and starts the planner threads.
    pub fn new(options: RustTransportOptions) -> Result<Self, CubeError> {
        let data_model = options.model_source.load_data_model()?;
        let meta = meta_context(
            &data_model,
            options.include_hidden_members,
            options.dialects.default,
            &options.dialects.by_data_source,
        )?;

        Ok(Self {
            meta,
            planner: PlannerPool::load(options.model_source, options.planner_threads)?,
            dialects: options.dialects,
            executor: options.executor,
            auth_config: options.auth_config,
            auth_service: options.auth_service,
        })
    }

    /// The compiled model as the SQL API sees it.
    pub fn meta_context(&self) -> Arc<MetaContext> {
        self.meta.clone()
    }

    /// The security context a request runs under, applying `__user` when the
    /// session switched to another user.
    ///
    /// cubesql has already asked [`TransportService::can_switch_user_for_session`]
    /// before it sets `change_user`, so this only has to produce the new
    /// user's context, which is what `contextByRequest` does in Node.
    async fn request_security_context(&self, session: &Session, meta: &LoadRequestMeta) -> Value {
        let Some(change_user) = meta.change_user() else {
            return session.security_context_value();
        };

        if Some(&change_user) == session.user.as_ref() {
            return session.security_context_value();
        }

        let Some(auth_service) = &self.auth_service else {
            // Without an auth service there is nowhere to get the other
            // user's claims from. The switch was already authorized, so the
            // request runs; it just keeps the session's own context.
            tracing::warn!(
                change_user = %change_user,
                "No SQL auth service is configured, so __user keeps the session's security context"
            );
            return session.security_context_value();
        };

        match auth_service
            .authenticate(
                SqlAuthServiceAuthenticateRequest {
                    protocol: "postgres".to_string(),
                    method: "password".to_string(),
                },
                Some(change_user.clone()),
                None,
            )
            .await
        {
            Ok(response) => response
                .context
                .security_context()
                .cloned()
                .unwrap_or(Value::Object(Map::new())),
            Err(e) => {
                tracing::warn!(
                    change_user = %change_user,
                    error = %e,
                    "Failed to build a security context for __user; keeping the session's"
                );
                session.security_context_value()
            }
        }
    }

    /// The `sqlApiLoad` request body, which is what an executor reads.
    #[allow(clippy::too_many_arguments)]
    fn load_request(
        request_id: String,
        query: &TransportLoadRequestQuery,
        sql_query: Option<&SqlQuery>,
        session: &Session,
        meta: &LoadRequestMeta,
        streaming: bool,
        cache_mode: Option<CacheMode>,
        span_id: Option<&Arc<SpanId>>,
    ) -> Result<Value, CubeError> {
        let mut request = Map::new();
        request.insert("id".to_string(), Value::String(request_id));
        request.insert(
            "meta".to_string(),
            serde_json::to_value(meta)
                .map_err(|e| CubeError::internal(format!("Failed to encode request meta: {e}")))?,
        );

        let mut body = Map::new();
        body.insert("request".to_string(), Value::Object(request));
        body.insert(
            "query".to_string(),
            serde_json::to_value(query)
                .map_err(|e| CubeError::internal(format!("Failed to encode the query: {e}")))?,
        );
        body.insert("session".to_string(), session.to_json());
        body.insert("streaming".to_string(), Value::Bool(streaming));
        if let Some(sql_query) = sql_query {
            body.insert(
                "sqlQuery".to_string(),
                json!([sql_query.sql.clone(), sql_query.values.clone()]),
            );
        }
        if let Some(cache_mode) = cache_mode {
            body.insert(
                "cacheMode".to_string(),
                serde_json::to_value(cache_mode).map_err(|e| {
                    CubeError::internal(format!("Failed to encode the cache mode: {e}"))
                })?,
            );
        }
        if let Some(span_id) = span_id {
            body.insert("queryKey".to_string(), span_id.query_key.clone());
        }

        Ok(Value::Object(body))
    }
}

#[async_trait]
impl TransportService for RustTransport {
    async fn meta(&self, _ctx: AuthContextRef) -> Result<Arc<MetaContext>, CubeError> {
        Ok(self.meta.clone())
    }

    async fn compiler_id(&self, _ctx: AuthContextRef) -> Result<Uuid, CubeError> {
        Ok(self.meta.compiler_id)
    }

    async fn sql(
        &self,
        span_id: Option<Arc<SpanId>>,
        query: TransportLoadRequestQuery,
        ctx: AuthContextRef,
        meta_fields: LoadRequestMeta,
        member_to_alias: Option<HashMap<String, String>>,
        expression_params: Option<Vec<Option<String>>>,
    ) -> Result<SqlResponse, CubeError> {
        if expression_params.as_ref().is_some_and(|p| !p.is_empty()) {
            return Err(CubeError::user(
                "SQL push-down with expression parameters is not supported by the Rust planner yet"
                    .to_string(),
            ));
        }

        let session = Session::from_context(&ctx);
        let security_context = self.request_security_context(&session, &meta_fields).await;
        let planner_query = to_planner_query(&query, member_to_alias)?;
        let options = plan_options(&self.dialects, security_context);

        tracing::debug!(
            request_id = %span_request_id(span_id.as_ref(), 1),
            "Planning SQL for the SQL API"
        );

        let (sql, values) = self.planner.plan(planner_query, options).await?;

        Ok(SqlResponse {
            sql: SqlQuery { sql, values },
        })
    }

    async fn load(
        &self,
        span_id: Option<Arc<SpanId>>,
        query: TransportLoadRequestQuery,
        sql_query: Option<SqlQuery>,
        ctx: AuthContextRef,
        meta_fields: LoadRequestMeta,
        schema: SchemaRef,
        member_fields: Vec<MemberField>,
        cache_mode: Option<CacheMode>,
        throw_continue_wait: bool,
    ) -> Result<Vec<RecordBatch>, CubeError> {
        let session = Session::from_context(&ctx);
        let security_context = self.request_security_context(&session, &meta_fields).await;

        let mut sequence: u32 = 0;
        loop {
            sequence += 1;
            let body = Self::load_request(
                span_request_id(span_id.as_ref(), sequence),
                &query,
                sql_query.as_ref(),
                &session,
                &meta_fields,
                false,
                cache_mode,
                span_id.as_ref(),
            )?;

            match self.executor.execute(body, &security_context).await {
                Ok(response) => {
                    return convert_transport_response(response, schema, member_fields);
                }
                Err(e) if e.is_continue_wait() => {
                    if throw_continue_wait {
                        return Err(CubeError::continue_wait());
                    }
                    tracing::debug!("SQL API load is retrying after a continue wait");
                    tokio::time::sleep(CONTINUE_WAIT_RETRY_DELAY).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn load_stream(
        &self,
        span_id: Option<Arc<SpanId>>,
        query: TransportLoadRequestQuery,
        sql_query: Option<SqlQuery>,
        ctx: AuthContextRef,
        meta_fields: LoadRequestMeta,
        schema: SchemaRef,
        member_fields: Vec<MemberField>,
        throw_continue_wait: bool,
    ) -> Result<CubeStreamReceiver, CubeError> {
        let session = Session::from_context(&ctx);
        let security_context = self.request_security_context(&session, &meta_fields).await;

        let mut sequence: u32 = 0;
        loop {
            sequence += 1;
            let body = Self::load_request(
                span_request_id(span_id.as_ref(), sequence),
                &query,
                sql_query.as_ref(),
                &session,
                &meta_fields,
                true,
                None,
                span_id.as_ref(),
            )?;

            match self
                .executor
                .execute_stream(
                    body,
                    &security_context,
                    schema.clone(),
                    member_fields.clone(),
                )
                .await
            {
                Ok(receiver) => return Ok(receiver),
                Err(e) if e.is_continue_wait() => {
                    if throw_continue_wait {
                        return Err(CubeError::continue_wait());
                    }
                    tracing::debug!("SQL API load_stream is retrying after a continue wait");
                    tokio::time::sleep(CONTINUE_WAIT_RETRY_DELAY).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn can_switch_user_for_session(
        &self,
        ctx: AuthContextRef,
        to_user: String,
    ) -> Result<bool, CubeError> {
        let session = Session::from_context(&ctx);

        // `canSwitchUserForSession` in Node: the superuser may always switch,
        // otherwise `canSwitchSqlUser` decides.
        Ok(session.superuser
            || self
                .auth_config
                .can_switch_user(session.user.as_deref(), &to_user))
    }

    async fn log_load_state(
        &self,
        span_id: Option<Arc<SpanId>>,
        ctx: AuthContextRef,
        meta_fields: LoadRequestMeta,
        event: String,
        properties: Value,
    ) -> Result<(), CubeError> {
        let session = Session::from_context(&ctx);

        // The redacted twin of the span's query travels beside `query`: the
        // log sink swaps it in, APM events keep the statement as sent.
        let mut properties = properties;
        if let Some(redacted_query) = span_id.as_ref().and_then(|s| s.redacted_query_key.clone()) {
            if let Some(object) = properties.as_object_mut() {
                if object.contains_key("query") {
                    object.insert("redactedQuery".to_string(), redacted_query);
                }
            }
        }

        let request_meta = serde_json::to_value(&meta_fields).unwrap_or(Value::Null);

        tracing::info!(
            target: "cube::sql_api",
            request_id = %span_request_id(span_id.as_ref(), 1),
            event = %event,
            user = session.user.as_deref().unwrap_or("-"),
            meta = %request_meta,
            properties = %properties,
            "SQL API load state"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::NotConfiguredExecutor;
    use cubesql::compile::engine::df::scan::Schema;
    use cubesql::transport::TransportLoadRequestQuery;

    const MODEL: &str = include_str!("../tests/model.yml");

    fn transport() -> RustTransport {
        RustTransport::new(RustTransportOptions {
            model_source: ModelSource::yaml(MODEL),
            planner_threads: 2,
            dialects: DialectMap::default(),
            include_hidden_members: false,
            executor: Arc::new(NotConfiguredExecutor::new()),
            auth_config: SqlAuthConfig {
                sql_user: Some("cube".to_string()),
                sql_password: Some("secret".to_string()),
                sql_super_user: Some("admin".to_string()),
                dev_mode: false,
            },
            auth_service: None,
        })
        .expect("the transport should build")
    }

    fn session(user: &str, superuser: bool) -> AuthContextRef {
        Arc::new(RustSqlAuthContext {
            user: Some(user.to_string()),
            superuser,
            security_context: Some(serde_json::json!({ "tenant_id": 7 })),
        })
    }

    fn request_meta() -> LoadRequestMeta {
        LoadRequestMeta::new("postgres".to_string(), "sql".to_string(), None)
    }

    fn query(measures: &[&str], dimensions: &[&str]) -> TransportLoadRequestQuery {
        let mut query = TransportLoadRequestQuery::new();
        query.measures = Some(measures.iter().map(|m| m.to_string()).collect());
        query.dimensions = Some(dimensions.iter().map(|d| d.to_string()).collect());
        query
    }

    #[tokio::test]
    async fn meta_comes_from_the_compiled_model() {
        let transport = transport();
        let meta = transport
            .meta(session("cube", false))
            .await
            .expect("meta should be available");

        assert_eq!(meta.cubes.len(), 3);
        assert_eq!(
            transport
                .compiler_id(session("cube", false))
                .await
                .expect("a compiler id"),
            meta.compiler_id
        );
    }

    #[tokio::test]
    async fn sql_is_planned_natively() {
        let transport = transport();

        let response = transport
            .sql(
                None,
                query(&["orders.count"], &["orders.status"]),
                session("cube", false),
                request_meta(),
                None,
                None,
            )
            .await
            .expect("the query should plan");

        let sql = response.sql.sql;
        assert!(sql.to_lowercase().contains("select"), "{sql}");
        assert!(sql.contains("public.orders"), "{sql}");
        assert!(sql.to_lowercase().contains("count("), "{sql}");
    }

    #[tokio::test]
    async fn sql_plans_a_join_across_cubes() {
        let transport = transport();

        let response = transport
            .sql(
                None,
                query(&["orders.count"], &["users.city"]),
                session("cube", false),
                request_meta(),
                None,
                None,
            )
            .await
            .expect("the join should plan");

        let sql = response.sql.sql;
        assert!(sql.contains("public.orders"), "{sql}");
        assert!(sql.contains("public.users"), "{sql}");
    }

    #[tokio::test]
    async fn sql_applies_the_member_aliases_the_sql_api_asks_for() {
        let transport = transport();

        let response = transport
            .sql(
                None,
                query(&["orders.count"], &[]),
                session("cube", false),
                request_meta(),
                Some(HashMap::from([(
                    "orders.count".to_string(),
                    "order_total".to_string(),
                )])),
                None,
            )
            .await
            .expect("the query should plan");

        assert!(
            response.sql.sql.contains("order_total"),
            "{}",
            response.sql.sql
        );
    }

    #[tokio::test]
    async fn a_query_the_planner_rejects_is_a_user_error() {
        let transport = transport();

        let error = transport
            .sql(
                None,
                query(&["orders.nonexistent"], &[]),
                session("cube", false),
                request_meta(),
                None,
                None,
            )
            .await
            .expect_err("an unknown member should be refused");
        assert!(!error.message.is_empty());
    }

    #[tokio::test]
    async fn push_down_expression_parameters_are_refused_by_name() {
        let transport = transport();

        let error = transport
            .sql(
                None,
                query(&["orders.count"], &[]),
                session("cube", false),
                request_meta(),
                None,
                Some(vec![Some("1".to_string())]),
            )
            .await
            .expect_err("expression params are not supported yet");
        assert!(
            error.message.contains("expression parameters"),
            "{}",
            error.message
        );
    }

    #[tokio::test]
    async fn load_without_an_executor_says_so() {
        let transport = transport();

        let error = transport
            .load(
                None,
                query(&["orders.count"], &[]),
                None,
                session("cube", false),
                request_meta(),
                Arc::new(Schema::empty()),
                vec![],
                None,
                false,
            )
            .await
            .expect_err("there is no executor");
        assert!(
            error.message.contains("query executor"),
            "{}",
            error.message
        );
    }

    #[tokio::test]
    async fn user_switching_follows_the_superuser_rules() {
        let transport = transport();

        // A regular user may only stay itself.
        assert!(transport
            .can_switch_user_for_session(session("cube", false), "cube".to_string())
            .await
            .unwrap());
        assert!(!transport
            .can_switch_user_for_session(session("cube", false), "other".to_string())
            .await
            .unwrap());

        // The configured superuser may switch, by name...
        assert!(transport
            .can_switch_user_for_session(session("admin", false), "other".to_string())
            .await
            .unwrap());
        // ...and a session the auth service already marked as a superuser may
        // too, whatever it is called.
        assert!(transport
            .can_switch_user_for_session(session("someone", true), "other".to_string())
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn log_load_state_does_not_need_a_sink_to_succeed() {
        let transport = transport();

        transport
            .log_load_state(
                None,
                session("cube", false),
                request_meta(),
                "Load Request".to_string(),
                serde_json::json!({ "query": "select 1" }),
            )
            .await
            .expect("logging should never fail the request");
    }

    #[test]
    fn a_request_id_names_its_span_exactly_once() {
        let span = Arc::new(SpanId::new(
            "abc".to_string(),
            serde_json::json!("select 1"),
        ));
        assert_eq!(span_request_id(Some(&span), 2), "abc-span-2");

        let already = Arc::new(SpanId::new(
            "abc-span-1".to_string(),
            serde_json::json!("select 1"),
        ));
        assert_eq!(span_request_id(Some(&already), 3), "abc-span-1");

        assert!(span_request_id(None, 1).ends_with("-span-1"));
    }
}
