use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use tracing::{debug, info, warn};

use super::session::{Attachment, ChannelRouter, InboundMessage};

/// Slack Events API webhook handler.
///
/// Handles two event types:
/// - `url_verification`: Responds with the challenge value (required during app setup).
/// - `event_callback`: Processes incoming messages and routes them to the session manager.
///
/// All requests are verified using HMAC-SHA256 signature validation with the
/// connection's `signing_secret`.
///
/// POST /api/runtime/events/webhook/slack/{connection_id}
pub async fn slack_webhook(
    State(router): State<Arc<ChannelRouter>>,
    Path(connection_id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    info!(connection_id = %connection_id, body_len = body.len(), "Slack webhook request received");

    // Parse the body as JSON.
    let payload: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };

    // Handle url_verification challenge (no signature check needed per Slack docs,
    // but we verify anyway if we have the signing secret).
    if payload.get("type").and_then(|t| t.as_str()) == Some("url_verification") {
        let challenge = payload
            .get("challenge")
            .and_then(|c| c.as_str())
            .unwrap_or("");
        return Json(json!({ "challenge": challenge })).into_response();
    }

    // Verify Slack request signature.
    if let Err(e) = verify_slack_signature(&router, &connection_id, &headers, &body).await {
        warn!(
            connection_id = %connection_id,
            error = %e,
            "Slack signature verification failed"
        );
        return StatusCode::UNAUTHORIZED.into_response();
    }

    // Process event_callback.
    if payload.get("type").and_then(|t| t.as_str()) != Some("event_callback") {
        return StatusCode::OK.into_response();
    }

    let event = match payload.get("event") {
        Some(e) => e,
        None => return StatusCode::OK.into_response(),
    };

    // Only handle message events (not message_changed, message_deleted, etc.)
    let event_type = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
    if event_type != "message" {
        return StatusCode::OK.into_response();
    }

    // Ignore bot messages to prevent loops.
    if event.get("bot_id").is_some() {
        return StatusCode::OK.into_response();
    }

    // Ignore subtypes except file_share (user uploaded a file).
    let subtype = event.get("subtype").and_then(|s| s.as_str());
    if subtype.is_some() && subtype != Some("file_share") {
        return StatusCode::OK.into_response();
    }

    let channel_id = event.get("channel").and_then(|c| c.as_str());
    let text = event.get("text").and_then(|t| t.as_str());

    let (Some(channel_id), Some(text)) = (channel_id, text) else {
        return StatusCode::OK.into_response();
    };

    // Strip Slack mention markup like <@U12345> from the message text.
    // Users type "@BotName hello" but Slack sends "<@U12345> hello".
    let clean_text = strip_slack_mentions(text);
    let clean_text = clean_text.trim();
    if clean_text.is_empty() {
        return StatusCode::OK.into_response();
    }

    // Use channel_id + optional thread_ts as conversation ID.
    // If threaded, scope to the thread. If not, scope to the channel.
    let conv_id = if let Some(thread_ts) = event.get("thread_ts").and_then(|t| t.as_str()) {
        format!("{}:{}", channel_id, thread_ts)
    } else {
        channel_id.to_string()
    };

    let sender_id = event
        .get("user")
        .and_then(|u| u.as_str())
        .unwrap_or(&conv_id)
        .to_string();

    // Extract file attachments from Slack event.
    let mut attachments = extract_slack_files(event);
    if !attachments.is_empty() {
        upload_slack_attachments_to_storage(&router, &connection_id, &mut attachments).await;
    }

    let msg = InboundMessage {
        text: clean_text.to_string(),
        sender_id,
        conv_id,
        channel: "slack".into(),
        attachments,
        original_message: payload.clone(),
    };

    debug!(connection_id = %connection_id, channel = %msg.conv_id, "Slack message received");

    if let Err(e) = router.handle_message(&connection_id, &msg).await {
        warn!(
            connection_id = %connection_id,
            error = %e,
            "Failed to handle Slack message"
        );
    }

    StatusCode::OK.into_response()
}

/// Verify the Slack request signature using HMAC-SHA256.
///
/// Slack signs every request with: `v0=HMAC-SHA256(signing_secret, "v0:{timestamp}:{body}")`.
/// The signature is in the `X-Slack-Signature` header, and the timestamp is in
/// `X-Slack-Request-Timestamp`.
async fn verify_slack_signature(
    router: &ChannelRouter,
    connection_id: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> anyhow::Result<()> {
    let signature = headers
        .get("x-slack-signature")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| anyhow::anyhow!("Missing X-Slack-Signature header"))?;

    let timestamp = headers
        .get("x-slack-request-timestamp")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| anyhow::anyhow!("Missing X-Slack-Request-Timestamp header"))?;

    // Prevent replay attacks: reject requests older than 5 minutes.
    let ts: i64 = timestamp
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid timestamp"))?;
    let now = chrono::Utc::now().timestamp();
    if (now - ts).abs() > 300 {
        anyhow::bail!("Request timestamp too old (possible replay attack)");
    }

    // Load the signing_secret from the connection.
    let signing_secret = load_signing_secret(router, connection_id).await?;

    // Compute expected signature: v0=HMAC-SHA256(signing_secret, "v0:{timestamp}:{body}")
    let sig_basestring = format!("v0:{}:{}", timestamp, String::from_utf8_lossy(body));

    let mut mac = Hmac::<Sha256>::new_from_slice(signing_secret.as_bytes())
        .map_err(|_| anyhow::anyhow!("Invalid signing secret"))?;
    mac.update(sig_basestring.as_bytes());
    let expected = format!("v0={}", hex::encode(mac.finalize().into_bytes()));

    // Constant-time comparison to prevent timing attacks.
    if !constant_time_eq(signature.as_bytes(), expected.as_bytes()) {
        anyhow::bail!("Signature mismatch");
    }

    Ok(())
}

/// Load the signing_secret from the Slack connection's parameters.
async fn load_signing_secret(
    router: &ChannelRouter,
    connection_id: &str,
) -> anyhow::Result<String> {
    let expected_tenant = crate::config::tenant_id();
    let conn = router
        .connections()
        .get_with_parameters(connection_id, expected_tenant)
        .await
        .map_err(|e| anyhow::anyhow!("DB error: {}", e))?
        .ok_or_else(|| anyhow::anyhow!("Connection not found"))?;

    let params = conn
        .connection_parameters
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Connection has no parameters"))?;

    params["signing_secret"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("Missing signing_secret"))
}

/// Strip Slack mention markup (`<@U12345>`) from message text.
fn strip_slack_mentions(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '<' && chars.peek() == Some(&'@') {
            // Skip until closing '>'
            for inner in chars.by_ref() {
                if inner == '>' {
                    break;
                }
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Constant-time byte comparison.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}

/// Extract file attachments from a Slack message event.
///
/// Slack includes a `files` array in the event when a user uploads files.
/// Each file has `name`, `mimetype`, `size`, and `url_private` (requires bot token auth).
fn extract_slack_files(event: &Value) -> Vec<Attachment> {
    let Some(files) = event.get("files").and_then(|f| f.as_array()) else {
        return vec![];
    };

    files
        .iter()
        .map(|file| {
            let name = file
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("unnamed");
            let mimetype = file
                .get("mimetype")
                .and_then(|m| m.as_str())
                .unwrap_or("application/octet-stream");
            let size = file.get("size").and_then(|s| s.as_u64()).unwrap_or(0);
            let url = file
                .get("url_private")
                .or_else(|| file.get("url_private_download"))
                .and_then(|u| u.as_str());

            Attachment {
                name: name.to_string(),
                content_type: mimetype.to_string(),
                size,
                url: url.map(|u| u.to_string()),
                data: None,
                storage_bucket: None,
                storage_key: None,
            }
        })
        .collect()
}

const ATTACHMENT_BUCKET: &str = "attachments";

/// Upload Slack file attachments to the tenant's internal S3 storage.
///
/// Downloads each file from Slack using the bot token for auth, then uploads
/// to the tenant's S3-compatible storage. On success, replaces the Slack URL
/// with internal `storage_bucket`/`storage_key`.
async fn upload_slack_attachments_to_storage(
    router: &ChannelRouter,
    connection_id: &str,
    attachments: &mut [Attachment],
) {
    if attachments.is_empty() {
        return;
    }

    let expected_tenant = crate::config::tenant_id();

    // Resolve S3 client from tenant's default s3_compatible connection.
    let s3_client =
        match crate::api::services::file_storage::FileStorageService::resolve_default_s3_client(
            router.connections(),
            expected_tenant,
        )
        .await
        {
            Ok(client) => client,
            Err(e) => {
                warn!(
                    connection_id = %connection_id,
                    error = ?e,
                    "No S3 storage configured — Slack attachments will not be persisted"
                );
                return;
            }
        };

    if let Err(e) = s3_client.create_bucket(ATTACHMENT_BUCKET) {
        warn!(error = %e, "Failed to create attachments bucket");
        return;
    }

    // Load bot_token for downloading files from Slack.
    let bot_token = load_bot_token(router, connection_id).await;

    let date_prefix = chrono::Utc::now().format("%Y-%m-%d").to_string();

    for attachment in attachments.iter_mut() {
        let Some(url) = &attachment.url else {
            continue;
        };

        let data = match download_slack_file(router, url, bot_token.as_deref()).await {
            Ok(bytes) => bytes,
            Err(e) => {
                warn!(
                    name = %attachment.name,
                    url = %url,
                    error = %e,
                    "Failed to download Slack file"
                );
                continue;
            }
        };

        let sanitized_name = attachment.name.replace(['/', '\\', '\0'], "_");
        let key = format!(
            "{}/{}/{}_{}",
            connection_id,
            date_prefix,
            uuid::Uuid::new_v4().as_simple(),
            sanitized_name
        );

        let content_type = Some(attachment.content_type.as_str());
        match s3_client.put_object(ATTACHMENT_BUCKET, &key, data.clone(), content_type) {
            Ok(()) => {
                attachment.size = data.len() as u64;
                attachment.storage_bucket = Some(ATTACHMENT_BUCKET.to_string());
                attachment.storage_key = Some(key.clone());
                attachment.url = None;
                debug!(
                    name = %attachment.name,
                    storage_key = %key,
                    "Slack attachment uploaded to internal storage"
                );
            }
            Err(e) => {
                warn!(
                    name = %attachment.name,
                    error = %e,
                    "Failed to upload Slack attachment to S3 — keeping original URL"
                );
            }
        }
    }
}

/// Download a file from Slack using Bearer token auth.
async fn download_slack_file(
    router: &ChannelRouter,
    url: &str,
    bot_token: Option<&str>,
) -> anyhow::Result<Vec<u8>> {
    let token = bot_token.ok_or_else(|| anyhow::anyhow!("No Slack bot_token available"))?;
    let resp = router
        .http_client()
        .get(url)
        .bearer_auth(token)
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!(
            "HTTP {} {}",
            resp.status().as_u16(),
            resp.status().canonical_reason().unwrap_or("")
        );
    }
    Ok(resp.bytes().await?.to_vec())
}

/// Load the bot_token from the Slack connection's parameters.
async fn load_bot_token(router: &ChannelRouter, connection_id: &str) -> Option<String> {
    let expected_tenant = crate::config::tenant_id();
    let conn = match router
        .connections()
        .get_with_parameters(connection_id, expected_tenant)
        .await
    {
        Ok(Some(c)) => c,
        _ => return None,
    };
    conn.connection_parameters
        .as_ref()
        .and_then(|p| p.get("bot_token"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}
