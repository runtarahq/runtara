//! Generic webhook signature verification for trigger-linked connections.
//!
//! When a trigger has a `connection_id` in its configuration, incoming webhook
//! requests are verified against the connection's credentials. Each integration
//! type defines its own verification method.

use axum::http::HeaderMap;
use runtara_connections::{ConnectionWithParameters, ConnectionsFacade, SignatureCheck};
use serde_json::Value;
use tracing::debug;

/// Verify a webhook request against the trigger's linked connection.
///
/// Returns Ok(()) if verification passes or if no verification is needed.
/// Returns Err with a message if verification fails.
pub async fn verify_webhook(
    facade: &ConnectionsFacade,
    trigger_config: &Option<Value>,
    tenant_id: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<(), String> {
    let connection_id = match trigger_config
        .as_ref()
        .and_then(|c| c.get("connection_id"))
        .and_then(|v| v.as_str())
    {
        Some(id) => id,
        None => return Ok(()),
    };

    let conn = match facade.get_with_parameters(connection_id, tenant_id).await {
        Ok(Some(c)) => c,
        Ok(None) => {
            debug!(connection_id = %connection_id, "Webhook verification skipped: connection not found");
            return Ok(());
        }
        Err(e) => {
            debug!(error = %e, "Webhook verification skipped: DB error");
            return Ok(());
        }
    };

    let integration_id = conn.integration_id.as_deref().unwrap_or("");
    if conn.connection_parameters.is_none() {
        return Ok(());
    }

    match integration_id {
        "mailgun" => verify_mailgun(facade, &conn, tenant_id, headers, body).await,
        _ => Ok(()),
    }
}

/// Verify Mailgun webhook signature.
///
/// Mailgun signs webhooks with HMAC-SHA256(signing_key, timestamp + token).
/// The signature fields are in the request body (JSON). A connection with no
/// signing key accepts the webhook unchecked; a sealed key is checked by seal.
async fn verify_mailgun(
    facade: &ConnectionsFacade,
    conn: &ConnectionWithParameters,
    tenant_id: &str,
    _headers: &HeaderMap,
    body: &[u8],
) -> Result<(), String> {
    let has_key = conn
        .connection_parameters
        .as_ref()
        .and_then(|params| params.get("webhook_signing_key"))
        .is_some_and(|key| !key.is_null() && key.as_str() != Some(""));
    if !has_key {
        return Ok(());
    }

    let body_value: Value = serde_json::from_slice(body).unwrap_or_default();

    // Mailgun webhook signature can be at top level or nested under "signature".
    let sig_obj = body_value.get("signature").unwrap_or(&body_value);

    let timestamp_str = match sig_obj.get("timestamp") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => return Err("Missing timestamp in webhook body".to_string()),
    };

    let token = sig_obj
        .get("token")
        .and_then(|v| v.as_str())
        .ok_or("Missing token in webhook body")?;

    let signature = sig_obj
        .get("signature")
        .and_then(|v| v.as_str())
        .ok_or("Missing signature in webhook body")?;

    let message = format!("{timestamp_str}{token}");
    match facade
        .check_webhook_signature(
            conn,
            tenant_id,
            "webhook_signing_key",
            message.as_bytes(),
            signature,
        )
        .await
        .map_err(|e| format!("Mailgun webhook signature could not be checked: {e}"))?
    {
        SignatureCheck::Valid | SignatureCheck::NoSecret => Ok(()),
        SignatureCheck::Invalid => Err("Mailgun webhook signature mismatch".to_string()),
    }
}
