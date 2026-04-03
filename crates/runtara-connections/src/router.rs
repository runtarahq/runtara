use crate::config::ConnectionsConfig;
use crate::handler;
use axum::{
    Router,
    routing::{delete, get, post, put},
};

/// CRUD + type discovery + rate limit analytics router.
/// The host app adds auth middleware and mounts at a chosen prefix.
pub fn connections_router(config: ConnectionsConfig) -> Router {
    Router::new()
        .route(
            "/connections",
            post(handler::connections::create_connection_handler),
        )
        .route(
            "/connections",
            get(handler::connections::list_connections_handler),
        )
        .route(
            "/connections/{id}",
            get(handler::connections::get_connection_handler),
        )
        .route(
            "/connections/{id}",
            put(handler::connections::update_connection_handler),
        )
        .route(
            "/connections/{id}",
            delete(handler::connections::delete_connection_handler),
        )
        .route(
            "/connections/operator/{operatorName}",
            get(handler::connections::get_connections_by_operator_handler),
        )
        .route(
            "/connections/categories",
            get(handler::connections::list_connection_categories_handler),
        )
        .route(
            "/connections/auth-types",
            get(handler::connections::list_connection_auth_types_handler),
        )
        .route(
            "/connections/types",
            get(handler::connections::list_connection_types_handler),
        )
        .route(
            "/connections/types/{integration_id}",
            get(handler::connections::get_connection_type_handler),
        )
        .route(
            "/connections/{id}/oauth/authorize",
            get(handler::oauth::authorize_handler),
        )
        .route(
            "/rate-limits",
            get(handler::rate_limits::list_rate_limits_handler),
        )
        .route(
            "/connections/{id}/rate-limit-status",
            get(handler::rate_limits::get_connection_rate_limit_status_handler),
        )
        .route(
            "/connections/{id}/rate-limit-history",
            get(handler::rate_limits::get_connection_rate_limit_history_handler),
        )
        .route(
            "/connections/{id}/rate-limit-timeline",
            get(handler::rate_limits::get_connection_rate_limit_timeline_handler),
        )
        .with_state(config.db_pool)
}

/// OAuth callback router (public, no auth).
pub fn oauth_callback_router(config: ConnectionsConfig) -> Router {
    Router::new()
        .route(
            "/{tenant_id}/callback",
            get(handler::oauth::callback_handler),
        )
        .with_state(config.db_pool)
}

/// Runtime credential resolution router (internal, no auth).
/// Tenant ID is extracted from the URL path.
pub fn runtime_router(config: ConnectionsConfig) -> Router {
    Router::new()
        .route(
            "/{tenant_id}/{connection_id}",
            get(handler::connections::get_connection_for_runtime_handler),
        )
        .with_state(config.db_pool)
}
