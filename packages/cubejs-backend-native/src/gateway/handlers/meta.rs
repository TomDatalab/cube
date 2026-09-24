use crate::gateway::auth_middleware::AuthExtension;
use crate::gateway::http_error::{HttpError, HttpErrorCode, HttpStatusCode};
use crate::gateway::state::ApiGatewayStateRef;
use crate::gateway::{GatewayMetaRequest, GatewayMetaService};
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum::{Extension, Json};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct MetaQueryParams {
    #[serde(rename = "onlyViews")]
    only_views: Option<String>,
    extended: Option<String>,
}

/// `GET /v1/meta` — same contract as the Node.js handler in
/// `packages/cubejs-api-gateway/src/gateway.ts`.
pub async fn meta_handler_v1(
    State(gateway_state): State<ApiGatewayStateRef>,
    Extension(auth): Extension<AuthExtension>,
    Query(params): Query<MetaQueryParams>,
) -> Result<impl IntoResponse, HttpError> {
    gateway_state
        .assert_api_scope(auth.auth_context(), "meta")
        .await?;

    // `?extended` is served by ApiGateway.metaExtended in Node.js and is not
    // exposed through the bridge yet.
    if params.extended.is_some() {
        return Err(HttpError::not_implemented(
            "/v1/meta?extended is not implemented in the native API gateway".to_string(),
        ));
    }

    let meta_service = gateway_state
        .injector_ref()
        .get_service_typed::<dyn GatewayMetaService>()
        .await;

    let response = meta_service
        .meta(
            auth.auth_context(),
            GatewayMetaRequest {
                // Node.js: `req.query.onlyViews === 'true'`
                only_views: params.only_views.as_deref() == Some("true"),
            },
        )
        .await
        .map_err(|err| {
            log::error!("Error loading meta: {}", err);

            HttpError::from_user_with_status_code(
                err,
                HttpErrorCode::StatusCode(HttpStatusCode::BAD_REQUEST),
            )
        })?;

    Ok(Json(response))
}
