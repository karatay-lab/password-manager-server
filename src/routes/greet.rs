use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::crypto::{encrypt, keys};
use crate::domain::identity::{Identity, NewIdentity};
use crate::error::{AppError, AppResult};
use crate::routes::AppState;

#[derive(Deserialize)]
pub struct GreetRequest {
    pub pub_key: String,
}

#[derive(Serialize)]
pub struct GreetResponse {
    pub server_public_key: String,
}

pub async fn greet_handler(
    State(state): State<Arc<AppState>>,
    ip: super::ClientIp,
    Json(req): Json<GreetRequest>,
) -> AppResult<Json<GreetResponse>> {
    let mut conn = state.conn()?;

    let client_key_bytes = hex::decode(&req.pub_key)
        .map_err(|_| AppError::Validation("invalid pub_key hex encoding".into()))?;
    if client_key_bytes.len() != 32 {
        return Err(AppError::Validation(
            "pub_key must be exactly 32 bytes (64 hex chars)".into(),
        ));
    }

    if Identity::find_by_ip(&mut conn, &ip.0)?.is_some() {
        return Err(AppError::PreconditionFailed("precondition failed".into()));
    }

    let identity_keys = keys::generate_identity_keys();

    let new_identity = NewIdentity {
        uuid: Uuid::new_v4().to_string(),
        user_id: None,
        ip_address: ip.0,
        device_token: None,
        server_private_key: encrypt::encrypt_db(
            &identity_keys.server_private_key,
            &state.config.database_encrypt_secret,
        )?,
        server_public_key: identity_keys.server_public_key.clone(),
        client_public_key: client_key_bytes,
        extra: "{}".to_string(),
        is_confirmed: false,
    };

    let identity = match Identity::create(&mut conn, new_identity) {
        Ok(identity) => identity,
        Err(diesel::result::Error::DatabaseError(
            diesel::result::DatabaseErrorKind::UniqueViolation,
            _,
        )) => return Err(AppError::PreconditionFailed("precondition failed".into())),
        Err(e) => return Err(e.into()),
    };

    tracing::info!(uuid = %identity.uuid, "new identity created via greet");

    Ok(Json(GreetResponse {
        server_public_key: hex::encode(identity_keys.server_public_key),
    }))
}
