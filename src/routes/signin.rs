use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use diesel::Connection;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use subtle::ConstantTimeEq;

use crate::crypto::{encrypt, hash_token, keys};
use crate::domain::identity::{Identity, UpdateIdentity};
use crate::domain::user::{NewUser, User};
use crate::error::{validate_length, AppError, AppResult};
use crate::routes::AppState;

const MAX_NAME_LEN: usize = 64;

#[derive(Deserialize)]
pub struct SignRequest {
    pub name: String,
    pub ehlo: String,
}

#[derive(Serialize)]
pub struct SignResponse {
    pub token: String,
}

fn decode_request(
    state: &AppState,
    conn: &mut diesel::SqliteConnection,
    ip: &str,
    req: &SignRequest,
) -> AppResult<(Identity, String, Vec<u8>)> {
    let identity = Identity::find_by_ip(conn, ip)?
        .ok_or_else(|| AppError::Unauthorized("unauthorized".into()))?;

    let server_private = encrypt::decrypt_db(
        &identity.server_private_key,
        &state.config.database_encrypt_secret,
    )?;
    let shared_key = keys::derive_shared_key(&server_private, &identity.client_public_key)?;

    let name_bytes =
        hex::decode(&req.name).map_err(|_| AppError::Validation("invalid name hex".into()))?;
    let ehlo_bytes =
        hex::decode(&req.ehlo).map_err(|_| AppError::Validation("invalid ehlo hex".into()))?;

    let decrypted_name = keys::decrypt_with_shared_key(&name_bytes, &shared_key)?;
    let decrypted_ehlo = keys::decrypt_with_shared_key(&ehlo_bytes, &shared_key)?;

    let name = String::from_utf8(decrypted_name)
        .map_err(|_| AppError::Validation("name must be valid utf8".into()))?;

    Ok((identity, name, decrypted_ehlo))
}

fn link_device(
    state: &AppState,
    conn: &mut diesel::SqliteConnection,
    identity_uuid: &str,
    user_id: &str,
) -> AppResult<String> {
    let token = Uuid::new_v4().to_string();
    Identity::update(
        conn,
        identity_uuid,
        UpdateIdentity {
            user_id: Some(user_id.to_string()),
            ip_address: None,
            device_token: Some(hash_token(&token)),
            client_public_key: None,
            extra: None,
            is_confirmed: Some(false),
        },
    )?;
    let _ = state;
    Ok(token)
}

fn validate_name(name: &str) -> AppResult<()> {
    if name.is_empty() {
        return Err(AppError::Validation("name must not be empty".into()));
    }
    validate_length("name", name, MAX_NAME_LEN)
}

pub async fn sign_up_handler(
    State(state): State<Arc<AppState>>,
    ip: super::ClientIp,
    Json(req): Json<SignRequest>,
) -> AppResult<Json<SignResponse>> {
    let mut conn = state.conn()?;

    let (identity, name, ehlo_bytes) = decode_request(&state, &mut conn, &ip.0, &req)?;
    validate_name(&name)?;

    let ehlo_secret = hex::encode(encrypt::encrypt_db(
        &ehlo_bytes,
        &state.config.database_encrypt_secret,
    )?);

    let (user_uuid, token) = conn.transaction::<_, AppError, _>(|conn| {
        if User::find_by_name(conn, &name)?.is_some() {
            return Err(AppError::Conflict("name already taken".into()));
        }
        let user = User::create(
            conn,
            NewUser {
                uuid: Uuid::new_v4().to_string(),
                name: name.clone(),
                ehlo_secret,
                is_deleted: false,
                extra: "{}".to_string(),
            },
        )?;
        let token = link_device(&state, conn, &identity.uuid, &user.uuid)?;
        Ok((user.uuid, token))
    })?;

    tracing::info!(uuid = %identity.uuid, user_id = %user_uuid, name = %name, "user signed up");
    Ok(Json(SignResponse { token }))
}

pub async fn sign_in_handler(
    State(state): State<Arc<AppState>>,
    ip: super::ClientIp,
    Json(req): Json<SignRequest>,
) -> AppResult<Json<SignResponse>> {
    let mut conn = state.conn()?;

    let (identity, name, ehlo_bytes) = decode_request(&state, &mut conn, &ip.0, &req)?;

    let user = User::find_by_name(&mut conn, &name)?
        .ok_or_else(|| AppError::Unauthorized("unauthorized".into()))?;
    if user.is_deleted {
        return Err(AppError::Unauthorized("unauthorized".into()));
    }

    let stored_ehlo_bytes = hex::decode(&user.ehlo_secret)
        .map_err(|_| AppError::Crypto("invalid ehlo storage format".into()))?;
    let decrypted_stored_ehlo =
        encrypt::decrypt_db(&stored_ehlo_bytes, &state.config.database_encrypt_secret)?;
    if ehlo_bytes.ct_ne(&decrypted_stored_ehlo).into() {
        return Err(AppError::Unauthorized("unauthorized".into()));
    }

    let token = link_device(&state, &mut conn, &identity.uuid, &user.uuid)?;

    tracing::info!(uuid = %identity.uuid, user_id = %user.uuid, name = %name, "device signed in");
    Ok(Json(SignResponse { token }))
}
