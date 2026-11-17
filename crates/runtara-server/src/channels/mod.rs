pub mod channel;
pub mod collector;
pub mod intake;
pub mod mailgun_webhook;
pub mod session;
pub mod slack_webhook;
pub mod teams_auth;
pub mod teams_webhook;
pub mod webhook;

#[cfg(all(
    test,
    feature = "db-integration-tests",
    feature = "valkey-integration-tests"
))]
mod intake_tests;

use std::sync::Arc;

use axum::{Router, routing::post};

/// Provider webhook routes for conversational channel triggers.
pub fn routes(router: Arc<session::ChannelRouter>) -> Router {
    Router::new()
        .route(
            "/api/runtime/events/webhook/telegram/{connection_id}",
            post(webhook::telegram_webhook),
        )
        .route(
            "/api/runtime/events/webhook/slack/{connection_id}",
            post(slack_webhook::slack_webhook),
        )
        .route(
            "/api/runtime/events/webhook/teams/{connection_id}",
            post(teams_webhook::teams_webhook),
        )
        .route(
            "/api/runtime/events/webhook/mailgun/{connection_id}",
            post(mailgun_webhook::mailgun_webhook),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            router.clone(),
            scope_connection_tenant,
        ))
        .with_state(router)
}

/// Runs a channel webhook in the tenant of the connection it names. The
/// request is unauthenticated and names no tenant unless its path does, so
/// the connection says whose it is; what the handler then reads and writes is
/// confined to that tenant's rows. A connection that is not found is left to
/// the handler to refuse.
async fn scope_connection_tenant(
    axum::extract::State(router): axum::extract::State<Arc<session::ChannelRouter>>,
    axum::extract::Path(connection_id): axum::extract::Path<String>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if runtara_store_postgres::tenant_scope::current().is_some() {
        return next.run(request).await;
    }
    match router.served_connection_tenant(&connection_id).await {
        Ok(tenant) => runtara_store_postgres::tenant_scope::scoped(tenant, next.run(request)).await,
        Err(_) => next.run(request).await,
    }
}
