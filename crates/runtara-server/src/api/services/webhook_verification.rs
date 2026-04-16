//! Generic webhook signature verification for trigger-linked connections.
//!
//! When a trigger has a `connection_id` in its configuration, incoming webhook
//! requests are verified against the connection's credentials. Each integration
//! type defines its own verification method.

use axum::http::HeaderMap;
use hmac::{Hmac, Mac};
use runtara_connections::ConnectionsFacade;
use serde_json::Value;
use sha2::Sha256;
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
    let params = match conn.connection_parameters.as_ref() {
        Some(p) => p,
        None => return Ok(()),
    };

    match integration_id {
        "mailgun" => verify_mailgun(params, headers, body),
        _ => Ok(()),
    }
}

/// Verify Mailgun webhook signature.
///
/// Mailgun signs webhooks with HMAC-SHA256(signing_key, timestamp + token).
/// The signature fields are in the request body (JSON).
fn verify_mailgun(params: &Value, _headers: &HeaderMap, body: &[u8]) -> Result<(), String> {
    let signing_key = match params.get("webhook_signing_key").and_then(|v| v.as_str()) {
        Some(key) if !key.is_empty() => key,
        _ => return Ok(()),
    };

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

    let mut mac = Hmac::<Sha256>::new_from_slice(signing_key.as_bytes())
        .map_err(|_| "Invalid signing key".to_string())?;
    mac.update(timestamp_str.as_bytes());
    mac.update(token.as_bytes());
    let expected = hex::encode(mac.finalize().into_bytes());

    if !constant_time_eq(signature.as_bytes(), expected.as_bytes()) {
        return Err("Mailgun webhook signature mismatch".to_string());
    }

    Ok(())
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}
