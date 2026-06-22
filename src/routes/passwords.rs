use std::sync::Arc;

use axum::extract::Path;
use axum::extract::Query;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::Config;
use crate::crypto::encrypt;
use crate::crypto::keys;
use crate::domain::group::Group;
use crate::domain::identity::Identity;
use crate::domain::password::{NewPassword, Password, UpdatePassword};
use crate::error::validate_length;
use crate::error::AppError;
use crate::error::AppResult;
use crate::routes::validate_auth;
use crate::routes::AppState;

#[derive(Default, Deserialize)]
pub struct ListParams {
    pub expired: Option<bool>,
    pub take: Option<i64>,
    pub size: Option<i64>,
}

#[derive(Serialize)]
pub struct PasswordEntry {
    pub uuid: String,
    pub pwd: String,
    pub expires: i64,
    pub created_at: String,
    pub updated_at: String,
    pub valid_since_days: i32,
}

#[derive(Serialize)]
pub struct PasswordDetail {
    pub uuid: String,
    pub pwd: String,
    pub name: String,
    pub extra: String,
    pub expires: i64,
    pub created_at: String,
    pub updated_at: String,
    pub valid_since_days: i32,
    pub group: GroupInfo,
}

#[derive(Serialize)]
pub struct GroupInfo {
    pub name: String,
    pub extra: String,
}

#[derive(Deserialize)]
pub struct CreatePasswordRequest {
    pub pwd: String,
    pub group_id: String,
    pub extra: Option<String>,
    pub name: Option<String>,
    pub valid_since_days: Option<i32>,
}

#[derive(Deserialize)]
pub struct UpdatePasswordRequest {
    pub pwd: String,
    pub group_id: String,
    pub extra: Option<String>,
    pub name: Option<String>,
}

fn encrypt_for_client(hex_data: &str, identity: &Identity, config: &Config) -> AppResult<String> {
    let data = hex::decode(hex_data).map_err(|_| AppError::Validation("invalid hex".into()))?;
    let db_decrypted = encrypt::decrypt_db(&data, &config.database_encrypt_secret)?;
    let server_private = encrypt::decrypt_db(
        &identity.server_private_key,
        &config.database_encrypt_secret,
    )?;
    let shared_key = keys::derive_shared_key(&server_private, &identity.client_public_key)?;
    let client_encrypted = keys::encrypt_with_shared_key(&db_decrypted, &shared_key)?;
    Ok(hex::encode(client_encrypted))
}

fn decrypt_from_client(hex_data: &str, identity: &Identity, config: &Config) -> AppResult<Vec<u8>> {
    let encrypted =
        hex::decode(hex_data).map_err(|_| AppError::Validation("invalid hex".into()))?;
    let server_private = encrypt::decrypt_db(
        &identity.server_private_key,
        &config.database_encrypt_secret,
    )?;
    let shared_key = keys::derive_shared_key(&server_private, &identity.client_public_key)?;
    keys::decrypt_with_shared_key(&encrypted, &shared_key)
}

fn days_remaining(valid_since: chrono::NaiveDateTime, valid_since_days: i32) -> i64 {
    let now = chrono::Utc::now().naive_utc();
    if now < valid_since {
        return valid_since_days as i64;
    }
    let elapsed = (now - valid_since).num_days();
    (valid_since_days as i64 - elapsed).max(0)
}

fn is_expired(valid_since: chrono::NaiveDateTime, valid_since_days: i32) -> bool {
    days_remaining(valid_since, valid_since_days) <= 0
}

pub async fn list_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ip: super::ClientIp,
    Query(params): Query<ListParams>,
) -> AppResult<Json<Vec<PasswordEntry>>> {
    let expired = params.expired.unwrap_or(false);
    let identity = validate_auth(&headers, &state, &ip.0).await?;
    let user_id = identity
        .user_id
        .clone()
        .ok_or_else(|| AppError::Unauthorized("unauthorized".into()))?;
    let mut conn = state.conn()?;

    let groups = Group::find_by_user(&mut conn, &user_id)?;
    let group_ids: Vec<String> = groups.iter().map(|g| g.uuid.clone()).collect();
    let passwords = Password::find_by_groups(&mut conn, &group_ids)?;

    let size = params.size.unwrap_or(50).clamp(1, 200);
    let take = params.take.unwrap_or(0).max(0);

    let entries = passwords
        .into_iter()
        .filter(|p| is_expired(p.valid_since, p.valid_since_days) == expired)
        .skip(take as usize)
        .take(size as usize)
        .map(|p| {
            let pwd_hex = encrypt_for_client(&p.pwd, &identity, &state.config)?;
            Ok(PasswordEntry {
                uuid: p.uuid,
                pwd: pwd_hex,
                expires: if expired {
                    0
                } else {
                    days_remaining(p.valid_since, p.valid_since_days)
                },
                created_at: p.created_at.to_string(),
                updated_at: p.updated_at.to_string(),
                valid_since_days: p.valid_since_days,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;

    Ok(Json(entries))
}

pub async fn create_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ip: super::ClientIp,
    Json(req): Json<CreatePasswordRequest>,
) -> AppResult<Json<PasswordEntry>> {
    validate_length(
        "password name",
        req.name.as_deref().unwrap_or_default(),
        256,
    )?;
    if let Some(ref extra) = req.extra {
        validate_length("password extra", extra, 4096)?;
    }

    let identity = validate_auth(&headers, &state, &ip.0).await?;
    let user_id = identity
        .user_id
        .clone()
        .ok_or_else(|| AppError::Unauthorized("unauthorized".into()))?;
    let mut conn = state.conn()?;

    let decrypted_pwd = decrypt_from_client(&req.pwd, &identity, &state.config)?;
    let db_encrypted = encrypt::encrypt_db(&decrypted_pwd, &state.config.database_encrypt_secret)?;
    let db_encrypted_hex = hex::encode(db_encrypted);

    let group = Group::find_by_uuid(&mut conn, &req.group_id)?
        .ok_or_else(|| AppError::Unauthorized("unauthorized".into()))?;
    if group.user_id != user_id {
        return Err(AppError::Unauthorized("unauthorized".into()));
    }

    let password = Password::create(
        &mut conn,
        NewPassword {
            uuid: Uuid::new_v4().to_string(),
            group_id: req.group_id,
            pwd: db_encrypted_hex,
            name: req.name.unwrap_or_default(),
            extra: req.extra.unwrap_or_else(|| "{}".to_string()),
            valid_since_days: req.valid_since_days.map(|d| d.clamp(1, 365)).unwrap_or(30),
            valid_since: chrono::Utc::now().naive_utc(),
        },
    )?;

    let pwd_for_client = encrypt_for_client(&password.pwd, &identity, &state.config)?;

    Ok(Json(PasswordEntry {
        uuid: password.uuid,
        pwd: pwd_for_client,
        expires: days_remaining(password.valid_since, password.valid_since_days),
        created_at: password.created_at.to_string(),
        updated_at: password.updated_at.to_string(),
        valid_since_days: password.valid_since_days,
    }))
}

pub async fn update_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ip: super::ClientIp,
    Path(uuid): Path<Uuid>,
    Json(req): Json<UpdatePasswordRequest>,
) -> AppResult<Json<()>> {
    validate_length(
        "password name",
        req.name.as_deref().unwrap_or_default(),
        256,
    )?;
    if let Some(ref extra) = req.extra {
        validate_length("password extra", extra, 4096)?;
    }

    let identity = validate_auth(&headers, &state, &ip.0).await?;
    let user_id = identity
        .user_id
        .clone()
        .ok_or_else(|| AppError::Unauthorized("unauthorized".into()))?;
    let mut conn = state.conn()?;
    let uuid_str = uuid.to_string();

    let existing = Password::find_by_uuid(&mut conn, &uuid_str)?
        .ok_or_else(|| AppError::Unauthorized("unauthorized".into()))?;
    let group = Group::find_by_uuid(&mut conn, &existing.group_id)?
        .ok_or_else(|| AppError::Unauthorized("unauthorized".into()))?;
    if group.user_id != user_id {
        return Err(AppError::Unauthorized("unauthorized".into()));
    }

    let target_group = Group::find_by_uuid(&mut conn, &req.group_id)?
        .ok_or_else(|| AppError::Unauthorized("unauthorized".into()))?;
    if target_group.user_id != user_id {
        return Err(AppError::Unauthorized("unauthorized".into()));
    }

    let decrypted_pwd = decrypt_from_client(&req.pwd, &identity, &state.config)?;
    let db_encrypted = encrypt::encrypt_db(&decrypted_pwd, &state.config.database_encrypt_secret)?;
    let db_encrypted_hex = hex::encode(db_encrypted);

    Password::update(
        &mut conn,
        &uuid_str,
        UpdatePassword {
            pwd: Some(db_encrypted_hex),
            group_id: Some(req.group_id),
            name: req.name,
            extra: req.extra,
            valid_since_days: None,
        },
    )?;

    Ok(Json(()))
}

pub async fn get_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ip: super::ClientIp,
    Path(uuid): Path<Uuid>,
) -> AppResult<Json<PasswordDetail>> {
    let identity = validate_auth(&headers, &state, &ip.0).await?;
    let user_id = identity
        .user_id
        .clone()
        .ok_or_else(|| AppError::Unauthorized("unauthorized".into()))?;
    let mut conn = state.conn()?;
    let uuid_str = uuid.to_string();

    let password = Password::find_by_uuid(&mut conn, &uuid_str)?
        .ok_or_else(|| AppError::Unauthorized("unauthorized".into()))?;
    let group = Group::find_by_uuid(&mut conn, &password.group_id)?
        .ok_or_else(|| AppError::Unauthorized("unauthorized".into()))?;
    if group.user_id != user_id {
        return Err(AppError::Unauthorized("unauthorized".into()));
    }

    let pwd_for_client = encrypt_for_client(&password.pwd, &identity, &state.config)?;

    Ok(Json(PasswordDetail {
        uuid: password.uuid,
        pwd: pwd_for_client,
        name: password.name,
        extra: password.extra,
        expires: days_remaining(password.valid_since, password.valid_since_days),
        created_at: password.created_at.to_string(),
        updated_at: password.updated_at.to_string(),
        valid_since_days: password.valid_since_days,
        group: GroupInfo {
            name: group.name,
            extra: group.extra,
        },
    }))
}
