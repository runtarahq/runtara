use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use serde_json::Value;
use tracing::{debug, warn};

use super::session::{ChannelRouter, InboundMessage};

/// Telegram webhook handler.
///
/// Receives Update objects from the Telegram Bot API. The connection_id
/// in the path identifies which bot/org/workflow this webhook belongs to.
/// The `X-Telegram-Bot-Api-Secret-Token` header is validated against the
/// webhook secret stored in the trigger's configuration.
///
/// POST /api/runtime/events/webhook/telegram/{connection_id}
pub async fn telegram_webhook(
    State(router): State<Arc<ChannelRouter>>,
    Path(connection_id): Path<String>,
    headers: HeaderMap,
    Json(update): Json<Value>,
) -> StatusCode {
    // Validate the secret token header from Telegram.
    let secret_header = headers
        .get("x-telegram-bot-api-secret-token")
        .and_then(|v| v.to_str().ok());

    if let Err(e) = router
        .validate_webhook_secret(&connection_id, secret_header)
        .await
    {
        warn!(
            connection_id = %connection_id,
            error = %e,
            "Webhook secret validation failed"
        );
        return StatusCode::UNAUTHORIZED;
    }

    let chat_id = update
        .get("message")
        .and_then(|m| m.get("chat"))
        .and_then(|c| c.get("id"))
        .and_then(|id| id.as_i64());

    let text = update
        .get("message")
        .and_then(|m| m.get("text"))
        .and_then(|t| t.as_str());

    let (Some(chat_id), Some(text)) = (chat_id, text) else {
        return StatusCode::OK;
    };

    let conv_id = chat_id.to_string();
    let sender_id = update
        .get("message")
        .and_then(|m| m.get("from"))
        .and_then(|f| f.get("id"))
        .and_then(|id| id.as_i64())
        .map(|id| id.to_string())
        .unwrap_or_else(|| conv_id.clone());

    // Telegram redelivers the SAME update_id if the webhook does not 200 in
    // time; use it as the dedup id.
    let activity_id = update.get("update_id").map(|v| v.to_string());

    let msg = InboundMessage {
        text: text.to_string(),
        sender_id,
        conv_id,
        channel: "telegram".into(),
        attachments: vec![],
        original_message: update.clone(),
        target: None,
        activity_id: activity_id.clone(),
    };

    // Deduplicate at-least-once redeliveries by update_id.
    if let Some(update_id) = activity_id.as_deref()
        && !router.reserve_activity(&connection_id, update_id).await
    {
        debug!(connection_id = %connection_id, update_id, "Dropping duplicate Telegram update");
        return StatusCode::OK;
    }

    debug!(connection_id = %connection_id, chat_id = %msg.conv_id, "Telegram message received");

    if let Err(e) = router.handle_message(&connection_id, &msg).await {
        warn!(
            connection_id = %connection_id,
            error = %e,
            "Failed to handle Telegram message"
        );
    }

    StatusCode::OK
}
