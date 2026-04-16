/// Errors returned by the connections service layer.
#[derive(Debug, thiserror::Error)]
pub enum ConnectionsError {
    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Redis error: {0}")]
    Redis(String),

    #[error("OAuth error: {0}")]
    OAuth(String),

    #[error("Auth resolution error: {0}")]
    AuthResolution(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

impl ConnectionsError {
    /// HTTP status code for this error.
    pub fn status_code(&self) -> axum::http::StatusCode {
        use axum::http::StatusCode;
        match self {
            Self::Validation(_) => StatusCode::BAD_REQUEST,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Redis(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::OAuth(_) => StatusCode::BAD_REQUEST,
            Self::AuthResolution(_) => StatusCode::BAD_GATEWAY,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}
