//! `/readyz` and `/livez` (`ApiGateway.readiness` / `liveness`).

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Serialize;

use crate::app::AppState;

#[derive(Serialize)]
struct HealthResponse {
    health: &'static str,
}

fn health_response(result: Result<(), String>, probe: &str) -> impl IntoResponse {
    match result {
        Ok(()) => (StatusCode::OK, Json(HealthResponse { health: "HEALTH" })),
        Err(error) => {
            tracing::error!(probe, error, "Internal Server Error on {} probe", probe);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(HealthResponse { health: "DOWN" }),
            )
        }
    }
}

pub async fn readiness(State(state): State<AppState>) -> impl IntoResponse {
    health_response(state.health.readiness().await, "readiness")
}

pub async fn liveness(State(state): State<AppState>) -> impl IntoResponse {
    health_response(state.health.liveness().await, "liveness")
}
