use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;

use crate::error::AppResult;
use crate::routes::validate_auth;
use crate::routes::AppState;

pub async fn verify_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ip: super::ClientIp,
) -> AppResult<Json<()>> {
    validate_auth(&headers, &state, &ip.0).await?;
    Ok(Json(()))
}
