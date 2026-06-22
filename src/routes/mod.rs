pub mod admin;
pub mod greet;
pub mod passwords;
pub mod refresh;
pub mod resign;
pub mod signin;
pub mod verify;

pub mod group;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::RwLock;
use std::time::Duration;

use axum::extract::ConnectInfo;
use axum::extract::FromRef;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use axum::response::IntoResponse;
use tower_governor::{governor::GovernorConfigBuilder, GovernorLayer};

use subtle::ConstantTimeEq;

use crate::config::Config;
use crate::crypto::hash_token;
use crate::db::{DbConn, DbPool};
use crate::domain::identity::Identity;
use crate::domain::user::User;
use crate::error::AppError;
use crate::error::AppResult;

#[derive(Clone)]
pub struct AppState {
    pub pool: Arc<RwLock<DbPool>>,
    pub config: Config,
}

impl AppState {
    pub fn conn(&self) -> AppResult<DbConn> {
        let pool = self.pool.read().expect("db pool lock poisoned").clone();
        Ok(pool.get()?)
    }

    pub fn reload_pool(&self) -> AppResult<()> {
        let fresh = crate::db::init_pool(&self.config.database_url, self.config.db_pool_size)?;
        *self.pool.write().expect("db pool lock poisoned") = fresh;
        Ok(())
    }

    pub fn swap_database(&self, staged_path: &str) -> AppResult<()> {
        let mut guard = self.pool.write().expect("db pool lock poisoned");
        std::fs::rename(staged_path, &self.config.database_url)
            .map_err(|e| AppError::Internal(format!("rename db failed: {e}")))?;
        *guard = crate::db::init_pool(&self.config.database_url, self.config.db_pool_size)?;
        Ok(())
    }
}

impl FromRef<Arc<AppState>> for AppState {
    fn from_ref(state: &Arc<AppState>) -> Self {
        (**state).clone()
    }
}

pub struct ClientIp(pub String);

impl<S> FromRequestParts<S> for ClientIp
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let ConnectInfo(addr) = *parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .ok_or((StatusCode::BAD_REQUEST, "missing connect info"))?;
        Ok(ClientIp(addr.ip().to_string()))
    }
}

pub fn check_admin_ip(addr: std::net::SocketAddr, state: &AppState) -> AppResult<()> {
    let cidr = &state.config.admin_allowed_subnet;
    let (base_str, prefix_str) = cidr.split_once('/').unwrap_or((cidr, "32"));
    let base: std::net::Ipv4Addr = base_str
        .parse()
        .map_err(|_| AppError::Internal("invalid ADMIN_ALLOWED_SUBNET".into()))?;
    let prefix: u8 = prefix_str
        .parse()
        .map_err(|_| AppError::Internal("invalid ADMIN_ALLOWED_SUBNET prefix".into()))?;
    if prefix > 32 {
        return Err(AppError::Internal(
            "ADMIN_ALLOWED_SUBNET prefix must be 0-32".into(),
        ));
    }
    let ip = match addr.ip() {
        std::net::IpAddr::V4(v4) => v4,
        _ => return Err(AppError::Unauthorized("unauthorized".into())),
    };
    let mask = if prefix == 0 {
        0u32
    } else {
        !0u32 << (32 - prefix)
    };
    if (u32::from(ip) & mask) == (u32::from(base) & mask) {
        Ok(())
    } else {
        Err(AppError::Unauthorized("unauthorized".into()))
    }
}

pub fn check_admin_key(headers: &HeaderMap, state: &AppState) -> AppResult<()> {
    let key = headers
        .get("admin-key")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::Unauthorized("unauthorized".into()))?;
    let expected = hex::encode(state.config.software_secret);
    if key.as_bytes().ct_ne(expected.as_bytes()).into() {
        return Err(AppError::Unauthorized("unauthorized".into()));
    }
    Ok(())
}

pub async fn validate_auth(headers: &HeaderMap, state: &AppState, ip: &str) -> AppResult<Identity> {
    let device_token = headers
        .get("device-token")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| AppError::Unauthorized("unauthorized".into()))?;

    let mut conn = state.conn()?;
    let identity = Identity::find_by_device_token(&mut conn, &hash_token(device_token))?
        .ok_or_else(|| AppError::Unauthorized("unauthorized".into()))?;

    if identity.ip_address != ip {
        return Err(AppError::Unauthorized("unauthorized".into()));
    }
    if !identity.is_confirmed {
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

    Ok(identity)
}

async fn health() -> impl IntoResponse {
    StatusCode::OK
}

pub fn build_router(state: Arc<AppState>) -> Router {
    let cors = match state.config.cors_origin.as_deref() {
        Some(origin) => match origin.parse::<axum::http::HeaderValue>() {
            Ok(value) => CorsLayer::new()
                .allow_origin(value)
                .allow_methods([
                    axum::http::Method::GET,
                    axum::http::Method::POST,
                    axum::http::Method::PUT,
                    axum::http::Method::DELETE,
                ])
                .allow_headers([
                    axum::http::header::CONTENT_TYPE,
                    axum::http::header::AUTHORIZATION,
                    axum::http::HeaderName::from_static("device-token"),
                    axum::http::HeaderName::from_static("admin-key"),
                ]),
            Err(_) => {
                tracing::error!(
                    cors_origin = %origin,
                    "CORS_ORIGIN is not a valid HTTP header value; cross-origin \
                     browser requests will be blocked. Fix CORS_ORIGIN to enable one."
                );
                CorsLayer::new()
            }
        },
        None => {
            tracing::warn!(
                "CORS_ORIGIN not set; cross-origin browser requests will be blocked. \
                 Set CORS_ORIGIN to allow a specific browser origin."
            );
            CorsLayer::new()
        }
    };

    let strict_config = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(2)
            .burst_size(5)
            .use_headers()
            .finish()
            .unwrap(),
    );

    let moderate_config = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(10)
            .burst_size(20)
            .use_headers()
            .finish()
            .unwrap(),
    );

    let admin_config = Arc::new(
        GovernorConfigBuilder::default()
            .per_second(5)
            .burst_size(10)
            .use_headers()
            .finish()
            .unwrap(),
    );

    let limiters = [
        strict_config.limiter().clone(),
        moderate_config.limiter().clone(),
        admin_config.limiter().clone(),
    ];
    std::thread::spawn(move || {
        let interval = std::time::Duration::from_secs(60);
        loop {
            std::thread::sleep(interval);
            let total: usize = limiters.iter().map(|l| l.len()).sum();
            tracing::info!("rate limiting storage size: {total}");
            for limiter in &limiters {
                limiter.retain_recent();
            }
        }
    });

    let public = Router::new()
        .route("/greet", post(greet::greet_handler))
        .route("/sign-up", post(signin::sign_up_handler))
        .route("/sign-in", post(signin::sign_in_handler))
        .layer(GovernorLayer {
            config: strict_config,
        });

    let admin = Router::new()
        .route("/admin/pending", get(group::pending_handler))
        .route("/admin/approve/{uuid}", post(group::approve_handler))
        .route("/admin/identities", get(admin::list_identities))
        .route("/admin/identities/{uuid}", delete(admin::delete_identity))
        .route(
            "/admin/identities/{uuid}/{action}",
            post(admin::toggle_identity),
        )
        .route("/admin/users", get(admin::list_users))
        .route("/admin/users/{uuid}/{action}", post(admin::toggle_user))
        .route("/admin/export", get(admin::export_handler))
        .route("/admin/import", post(admin::import_handler))
        .layer(GovernorLayer {
            config: admin_config,
        });

    let general = Router::new()
        .route("/health", get(health))
        .route("/re-sign", post(resign::resign_handler))
        .route("/refresh", post(refresh::refresh_handler))
        .route("/verify", get(verify::verify_handler))
        .route("/pwd/list", get(passwords::list_handler))
        .route("/pwd/create", post(passwords::create_handler))
        .route("/pwd/update/{uuid}", put(passwords::update_handler))
        .route("/pwd/get/{uuid}", get(passwords::get_handler))
        .route("/group/create", post(group::create_handler))
        .route("/group/list", get(group::list_handler))
        .layer(GovernorLayer {
            config: moderate_config,
        });

    Router::new()
        .merge(public)
        .merge(admin)
        .merge(general)
        .layer(TraceLayer::new_for_http())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(RequestBodyLimitLayer::new(1024 * 1024))
        .layer(cors)
        .with_state(state)
}
