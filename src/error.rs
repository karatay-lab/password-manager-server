use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("identity not found")]
    IdentityNotFound,

    #[error("not found: {0}")]
    NotFound(String),

    #[error("unauthorized: {0}")]
    Unauthorized(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("precondition failed: {0}")]
    PreconditionFailed(String),

    #[error("database error: {0}")]
    Database(#[from] diesel::result::Error),

    #[error("connection pool error: {0}")]
    Pool(#[from] diesel::r2d2::PoolError),

    #[error("crypto error: {0}")]
    Crypto(String),

    #[error("internal error: {0}")]
    Internal(String),

    #[error("validation error: {0}")]
    Validation(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match &self {
            AppError::IdentityNotFound => StatusCode::NOT_FOUND,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::PreconditionFailed(_) => StatusCode::PRECONDITION_FAILED,
            AppError::Validation(_) => StatusCode::BAD_REQUEST,
            AppError::Database(_)
            | AppError::Pool(_)
            | AppError::Crypto(_)
            | AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, self.to_string()).into_response()
    }
}

pub fn validate_length(name: &str, field: &str, max: usize) -> AppResult<()> {
    if field.len() > max {
        return Err(AppError::Validation(format!(
            "{name} exceeds {max} characters"
        )));
    }
    Ok(())
}

pub type AppResult<T> = Result<T, AppError>;
