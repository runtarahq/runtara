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
        .with_state(router)
}
