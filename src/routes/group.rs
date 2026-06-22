use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::ConnectInfo;
use axum::extract::Path;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::group::{Group, NewGroup};
use crate::domain::identity::Identity;
use crate::error::validate_length;
use crate::error::AppResult;
use crate::routes::admin;
use crate::routes::AppState;
use crate::routes::{check_admin_ip, check_admin_key, validate_auth};

#[derive(Serialize)]
pub struct GroupResponse {
    pub uuid: String,
    pub name: String,
    pub extra: String,
}

#[derive(Deserialize)]
pub struct CreateGroupRequest {
    pub name: String,
    pub extra: Option<String>,
}

pub async fn create_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ip: super::ClientIp,
    Json(req): Json<CreateGroupRequest>,
) -> AppResult<Json<GroupResponse>> {
    validate_length("group name", &req.name, 128)?;

    let identity = validate_auth(&headers, &state, &ip.0).await?;
    let user_id = identity
        .user_id
        .clone()
        .ok_or_else(|| crate::error::AppError::Unauthorized("unauthorized".into()))?;
    let mut conn = state.conn()?;

    let group = Group::create(
        &mut conn,
        NewGroup {
            uuid: Uuid::new_v4().to_string(),
            user_id,
            name: req.name,
            extra: req.extra.unwrap_or_else(|| "{}".to_string()),
        },
    )?;

    Ok(Json(GroupResponse {
        uuid: group.uuid,
        name: group.name,
        extra: group.extra,
    }))
}

pub async fn list_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ip: super::ClientIp,
) -> AppResult<Json<Vec<GroupResponse>>> {
    let identity = validate_auth(&headers, &state, &ip.0).await?;
    let user_id = identity
        .user_id
        .clone()
        .ok_or_else(|| crate::error::AppError::Unauthorized("unauthorized".into()))?;
    let mut conn = state.conn()?;

    let groups = Group::find_by_user(&mut conn, &user_id)?;
    let entries = groups
        .into_iter()
        .map(|g| GroupResponse {
            uuid: g.uuid,
            name: g.name,
            extra: g.extra,
        })
        .collect();

    Ok(Json(entries))
}

#[derive(Serialize)]
pub struct PendingIdentity {
    pub uuid: String,
    pub ip_address: String,
}

pub async fn pending_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> AppResult<Json<Vec<PendingIdentity>>> {
    check_admin_key(&headers, &state)?;
    check_admin_ip(addr, &state)?;
    let mut conn = state.conn()?;

    let pending = Identity::find_pending(&mut conn)?;
    Ok(Json(
        pending
            .into_iter()
            .map(|i| PendingIdentity {
                uuid: i.uuid,
                ip_address: i.ip_address,
            })
            .collect(),
    ))
}

pub async fn approve_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(uuid): Path<Uuid>,
) -> AppResult<Json<()>> {
    check_admin_key(&headers, &state)?;
    check_admin_ip(addr, &state)?;
    let uuid_str = uuid.to_string();
    admin::set_confirm(&state, &uuid_str, true)?;
    tracing::info!(uuid = %uuid_str, "identity approved by admin");
    Ok(Json(()))
}
