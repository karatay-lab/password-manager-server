use std::io::Read;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::ConnectInfo;
use axum::extract::Path;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::Json;
use diesel::Connection;
use serde::Serialize;
use uuid::Uuid;

use crate::domain::identity::{Identity, UpdateIdentity};
use crate::domain::user::{UpdateUser, User};
use crate::error::{AppError, AppResult};
use crate::routes::AppState;
use crate::routes::{check_admin_ip, check_admin_key};

#[derive(Serialize)]
pub struct IdentityResponse {
    pub uuid: String,
    pub user_id: Option<String>,
    pub ip_address: String,
    pub device_token: Option<String>,
    pub is_confirmed: bool,
    pub created_at: String,
    pub updated_at: String,
}

pub async fn list_identities(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> AppResult<Json<Vec<IdentityResponse>>> {
    check_admin_key(&headers, &state)?;
    check_admin_ip(addr, &state)?;
    let mut conn = state.conn()?;
    let all = Identity::find_all(&mut conn)?;
    Ok(Json(
        all.into_iter()
            .map(|i| IdentityResponse {
                uuid: i.uuid,
                user_id: i.user_id,
                ip_address: i.ip_address,
                device_token: i.device_token,
                is_confirmed: i.is_confirmed,
                created_at: i.created_at.to_string(),
                updated_at: i.updated_at.to_string(),
            })
            .collect(),
    ))
}

pub fn set_confirm(state: &AppState, uuid_str: &str, confirmed: bool) -> AppResult<()> {
    let mut conn = state.conn()?;
    Identity::find_by_uuid(&mut conn, uuid_str)?.ok_or_else(|| AppError::IdentityNotFound)?;
    Identity::update(
        &mut conn,
        uuid_str,
        UpdateIdentity {
            user_id: None,
            ip_address: None,
            device_token: None,
            client_public_key: None,
            extra: None,
            is_confirmed: Some(confirmed),
        },
    )?;
    Ok(())
}

pub async fn delete_identity(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(uuid): Path<Uuid>,
) -> AppResult<Json<()>> {
    check_admin_key(&headers, &state)?;
    check_admin_ip(addr, &state)?;
    let uuid_str = uuid.to_string();
    let mut conn = state.conn()?;

    Identity::find_by_uuid(&mut conn, &uuid_str)?.ok_or(AppError::IdentityNotFound)?;

    Identity::delete(&mut conn, &uuid_str)?;

    tracing::info!(uuid = %uuid_str, "identity (device) deleted by admin");
    Ok(Json(()))
}

#[derive(Serialize)]
pub struct UserResponse {
    pub uuid: String,
    pub name: String,
    pub is_deleted: bool,
    pub identity_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

pub async fn list_users(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> AppResult<Json<Vec<UserResponse>>> {
    check_admin_key(&headers, &state)?;
    check_admin_ip(addr, &state)?;
    let mut conn = state.conn()?;

    let users = User::find_all(&mut conn)?;
    let identities = Identity::find_all(&mut conn)?;
    let mut counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    for i in &identities {
        if let Some(uid) = &i.user_id {
            *counts.entry(uid.clone()).or_insert(0) += 1;
        }
    }

    Ok(Json(
        users
            .into_iter()
            .map(|u| {
                let identity_count = *counts.get(&u.uuid).unwrap_or(&0);
                UserResponse {
                    uuid: u.uuid,
                    name: u.name,
                    is_deleted: u.is_deleted,
                    identity_count,
                    created_at: u.created_at.to_string(),
                    updated_at: u.updated_at.to_string(),
                }
            })
            .collect(),
    ))
}

pub async fn toggle_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path((uuid, action)): Path<(Uuid, String)>,
) -> AppResult<Json<()>> {
    check_admin_key(&headers, &state)?;
    check_admin_ip(addr, &state)?;
    let uuid_str = uuid.to_string();
    let is_deleted = match action.as_str() {
        "delete" => true,
        "restore" => false,
        _ => {
            return Err(AppError::Validation(
                "action must be delete or restore".into(),
            ))
        }
    };

    let mut conn = state.conn()?;
    User::find_by_uuid(&mut conn, &uuid_str)?
        .ok_or_else(|| AppError::NotFound("user not found".into()))?;

    User::update(
        &mut conn,
        &uuid_str,
        UpdateUser {
            is_deleted: Some(is_deleted),
            extra: None,
        },
    )?;

    tracing::info!(uuid = %uuid_str, action = %action, "user soft-delete toggled by admin");
    Ok(Json(()))
}

pub async fn toggle_identity(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path((uuid, action)): Path<(Uuid, String)>,
) -> AppResult<Json<()>> {
    check_admin_key(&headers, &state)?;
    check_admin_ip(addr, &state)?;
    let uuid_str = uuid.to_string();
    let confirmed = match action.as_str() {
        "confirm" => true,
        "unconfirm" => false,
        _ => {
            return Err(AppError::Validation(
                "action must be confirm or unconfirm".into(),
            ))
        }
    };
    set_confirm(&state, &uuid_str, confirmed)?;
    tracing::info!(uuid = %uuid_str, action = %action, "identity toggled via admin CLI");
    Ok(Json(()))
}

pub async fn export_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> AppResult<(StatusCode, [(String, String); 1], Vec<u8>)> {
    check_admin_key(&headers, &state)?;
    check_admin_ip(addr, &state)?;

    let db_path = state.config.database_url.clone();
    let mut db_file = std::fs::File::open(&db_path)
        .map_err(|e| AppError::Internal(format!("failed to open db: {e}")))?;
    let mut db_data = Vec::new();
    db_file
        .read_to_end(&mut db_data)
        .map_err(|e| AppError::Internal(format!("failed to read db: {e}")))?;

    let enc_hex = hex::encode(state.config.database_encrypt_secret);
    let sw_hex = hex::encode(state.config.software_secret);
    let env_content = format!("DATABASEENCRYPTSECRET={enc_hex}\nSOFTWARESECRET={sw_hex}\n");
    let readme = format!(
        "Password Manager Export\n\
         Generated: {}\n\n\
         This archive contains:\n\
         - pwd_manager.db  (SQLite database, all passwords encrypted at rest)\n\
         - .env            (export secrets — KEEP SAFE)\n\n\
         To import, use the admin CLI Import tab or POST /admin/import.\n",
        chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
    );

    let mut buf = Vec::new();
    {
        let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
            &mut buf,
            flate2::Compression::default(),
        ));
        let mut header = tar::Header::new_gnu();
        header.set_path("pwd_manager.db").unwrap();
        header.set_size(db_data.len() as u64);
        header.set_cksum();
        tar.append(&header, &db_data[..])
            .map_err(|e| AppError::Internal(format!("tar append db failed: {e}")))?;

        let mut header = tar::Header::new_gnu();
        header.set_path(".env").unwrap();
        header.set_size(env_content.len() as u64);
        header.set_cksum();
        tar.append(&header, env_content.as_bytes())
            .map_err(|e| AppError::Internal(format!("tar append env failed: {e}")))?;

        let mut header = tar::Header::new_gnu();
        header.set_path("README.txt").unwrap();
        header.set_size(readme.len() as u64);
        header.set_cksum();
        tar.append(&header, readme.as_bytes())
            .map_err(|e| AppError::Internal(format!("tar append readme failed: {e}")))?;

        tar.finish()
            .map_err(|e| AppError::Internal(format!("tar finish failed: {e}")))?;
    }

    tracing::info!("database exported ({} bytes)", buf.len());
    Ok((
        StatusCode::OK,
        [("content-type".to_string(), "application/gzip".to_string())],
        buf,
    ))
}

pub async fn import_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    body: Bytes,
) -> AppResult<Json<()>> {
    check_admin_key(&headers, &state)?;
    check_admin_ip(addr, &state)?;

    let temp_dir =
        tempfile::tempdir().map_err(|e| AppError::Internal(format!("tempdir failed: {e}")))?;

    let gz = flate2::read::GzDecoder::new(&body[..]);
    let mut archive = tar::Archive::new(gz);
    archive
        .unpack(temp_dir.path())
        .map_err(|e| AppError::Internal(format!("tar unpack failed: {e}")))?;

    let env_path = temp_dir.path().join(".env");
    let env_str = std::fs::read_to_string(&env_path)
        .map_err(|_| AppError::Validation("missing .env in archive".into()))?;
    let db_path_import = temp_dir.path().join("pwd_manager.db");

    let mut enc_key = None;
    let mut sw_key = None;
    for line in env_str.lines() {
        if let Some(val) = line.strip_prefix("DATABASEENCRYPTSECRET=") {
            enc_key = Some(val.to_string());
        } else if let Some(val) = line.strip_prefix("SOFTWARESECRET=") {
            sw_key = Some(val.to_string());
        }
    }
    let (enc_key, sw_key) = match (enc_key, sw_key) {
        (Some(e), Some(s)) => (e, s),
        _ => return Err(AppError::Validation("incomplete .env in archive".into())),
    };

    use subtle::ConstantTimeEq;
    let current_enc = hex::encode(state.config.database_encrypt_secret);
    let current_sw = hex::encode(state.config.software_secret);
    if enc_key.as_bytes().ct_ne(current_enc.as_bytes()).into()
        || sw_key.as_bytes().ct_ne(current_sw.as_bytes()).into()
    {
        return Err(AppError::Unauthorized(
            "archive secrets do not match server configuration".into(),
        ));
    }

    let imported_db = std::fs::read(&db_path_import)
        .map_err(|_| AppError::Validation("missing pwd_manager.db in archive".into()))?;

    let db_path = &state.config.database_url;
    let temp_db = format!("{db_path}.importing");
    std::fs::write(&temp_db, &imported_db)
        .map_err(|e| AppError::Internal(format!("write temp db failed: {e}")))?;

    let _conn = diesel::sqlite::SqliteConnection::establish(&temp_db)
        .map_err(|_| AppError::Validation("imported db is not a valid SQLite database".into()))?;

    state.swap_database(&temp_db)?;

    {
        let pool = state.pool.read().expect("db pool lock poisoned").clone();
        crate::db::run_migrations(&pool)?;
    }

    tracing::info!("database imported, pool reloaded, and migrations applied successfully");
    Ok(Json(()))
}
