use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use subtle::ConstantTimeEq;

use crate::crypto::{encrypt, hash_token, keys};
use crate::domain::identity::{Identity, UpdateIdentity};
use crate::domain::user::User;
use crate::error::{AppError, AppResult};
use crate::routes::AppState;

#[derive(Deserialize)]
pub struct RefreshRequest {
    pub token: String,
    pub ehlo: String,
}

#[derive(Serialize)]
pub struct RefreshResponse {
    pub token: String,
}

pub async fn refresh_handler(
    State(state): State<Arc<AppState>>,
    ip: super::ClientIp,
    Json(req): Json<RefreshRequest>,
) -> AppResult<Json<RefreshResponse>> {
    let mut conn = state.conn()?;

    let identity = Identity::find_by_ip(&mut conn, &ip.0)?
        .ok_or_else(|| AppError::Unauthorized("unauthorized".into()))?;

    let server_private = encrypt::decrypt_db(
        &identity.server_private_key,
        &state.config.database_encrypt_secret,
    )?;
    let client_public = &identity.client_public_key;
    let shared_key = keys::derive_shared_key(&server_private, client_public)?;

    let token_bytes =
        hex::decode(&req.token).map_err(|_| AppError::Validation("invalid token hex".into()))?;
    let ehlo_bytes =
        hex::decode(&req.ehlo).map_err(|_| AppError::Validation("invalid ehlo hex".into()))?;

    let decrypted_token = keys::decrypt_with_shared_key(&token_bytes, &shared_key)?;
    let decrypted_ehlo = keys::decrypt_with_shared_key(&ehlo_bytes, &shared_key)?;

    let device_token_str = String::from_utf8(decrypted_token)
        .map_err(|_| AppError::Validation("token must be valid utf8".into()))?;

    let hashed_sent_token = hash_token(&device_token_str);
    let stored_token = identity.device_token.as_deref().unwrap_or("");
    if hashed_sent_token
        .as_bytes()
        .ct_ne(stored_token.as_bytes())
        .into()
    {
        return Err(AppError::Unauthorized("unauthorized".into()));
    }

    let user_id = identity
        .user_id
        .as_deref()
        .ok_or_else(|| AppError::Unauthorized("unauthorized".into()))?;
    let user = User::find_by_uuid(&mut conn, user_id)?
        .ok_or_else(|| AppError::Unauthorized("unauthorized".into()))?;
    if user.is_deleted {
        return Err(AppError::Unauthorized("unauthorized".into()));
    }

    let stored_ehlo_bytes = hex::decode(&user.ehlo_secret)
        .map_err(|_| AppError::Crypto("invalid ehlo storage format".into()))?;
    let decrypted_stored_ehlo =
        encrypt::decrypt_db(&stored_ehlo_bytes, &state.config.database_encrypt_secret)?;
    if decrypted_ehlo.ct_ne(&decrypted_stored_ehlo).into() {
        return Err(AppError::Unauthorized("unauthorized".into()));
    }

    let new_token = Uuid::new_v4().to_string();

    Identity::update(
        &mut conn,
        &identity.uuid,
        UpdateIdentity {
            user_id: None,
            ip_address: None,
            device_token: Some(hash_token(&new_token)),
            client_public_key: None,
            extra: None,
            is_confirmed: None,
        },
    )?;

    tracing::info!(uuid = %identity.uuid, "device token refreshed");

    Ok(Json(RefreshResponse { token: new_token }))
}
