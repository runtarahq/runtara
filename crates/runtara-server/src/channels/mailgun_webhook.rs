use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{FromRequest, Multipart, Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;
use tracing::{debug, warn};

use super::session::{Attachment, ChannelRouter, InboundMessage};

/// Mailgun inbound email webhook handler.
///
/// Mailgun routes forward inbound emails as application/x-www-form-urlencoded
/// with fields like: sender, subject, body-plain, from, timestamp, token, signature.
///
/// POST /api/runtime/events/webhook/mailgun/{connection_id}
pub async fn mailgun_webhook(
    State(router): State<Arc<ChannelRouter>>,
    Path(connection_id): Path<String>,
    request: Request,
) -> Response {
    let (parts, body) = request.into_parts();
    let headers = parts.headers.clone();
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    debug!(
        connection_id = %connection_id,
        content_type = %content_type,
        "Mailgun webhook received"
    );

    // Parse the body based on content type.
    let (fields, file_attachments) = if content_type.starts_with("multipart/form-data") {
        // Multipart mode (forward with attachments).
        let reconstructed = Request::from_parts(parts, body);
        match parse_multipart(reconstructed).await {
            Ok(result) => result,
            Err(e) => {
                warn!(error = %e, "Failed to parse multipart");
                return StatusCode::BAD_REQUEST.into_response();
            }
        }
    } else {
        // Try JSON or form-urlencoded.
        let bytes = match axum::body::to_bytes(body, 10_000_000).await {
            Ok(b) => b,
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        };

        let fields = if let Ok(json) = serde_json::from_slice::<Value>(&bytes) {
            let mut map = HashMap::new();
            if let Some(obj) = json.as_object() {
                for (k, v) in obj {
                    if let Some(s) = v.as_str() {
                        map.insert(k.clone(), s.to_string());
                    }
                }
            }
            if let Some(sender) = json
                .get("envelope")
                .and_then(|e| e.get("sender"))
                .and_then(|v| v.as_str())
            {
                map.entry("sender".into()).or_insert(sender.to_string());
            }
            if let Some(from) = json
                .get("message")
                .and_then(|m| m.get("headers"))
                .and_then(|h| h.get("from"))
                .and_then(|v| v.as_str())
            {
                map.entry("from".into()).or_insert(from.to_string());
            }
            if let Some(subject) = json
                .get("message")
                .and_then(|m| m.get("headers"))
                .and_then(|h| h.get("subject"))
                .and_then(|v| v.as_str())
            {
                map.entry("subject".into()).or_insert(subject.to_string());
            }
            if let Some(url) = json
                .get("storage")
                .and_then(|s| s.get("url"))
                .and_then(|u| {
                    u.as_str()
                        .map(|s| s.to_string())
                        .or_else(|| u.as_array()?.first()?.as_str().map(|s| s.to_string()))
                })
            {
                map.insert("storage-url".into(), url);
            }
            map
        } else {
            let body_str = String::from_utf8_lossy(&bytes);
            form_decode(&body_str)
        };

        (fields, Vec::new())
    };

    debug!(
        connection_id = %connection_id,
        field_count = fields.len(),
        has_sender = fields.contains_key("sender"),
        has_from = fields.contains_key("from"),
        has_body_plain = fields.contains_key("body-plain"),
        has_subject = fields.contains_key("subject"),
        "Mailgun webhook received"
    );

    // Verify Mailgun signature (fields: timestamp, token, signature).
    if let Err(e) = verify_mailgun_signature(&router, &connection_id, &fields).await {
        warn!(
            connection_id = %connection_id,
            error = %e,
            "Mailgun signature verification failed"
        );
        return StatusCode::UNAUTHORIZED.into_response();
    }

    // Extract sender.
    let sender = fields
        .get("sender")
        .or_else(|| fields.get("from"))
        .map(|s| s.as_str());

    let Some(sender) = sender else {
        warn!(connection_id = %connection_id, "No sender in Mailgun webhook");
        return StatusCode::OK.into_response();
    };

    let sender_email = extract_email(sender);

    let subject = fields
        .get("subject")
        .map(|s| s.as_str())
        .unwrap_or("(no subject)");

    // Get the email body — prefer stripped-text (no quoted replies) over body-plain.
    let body_plain = if let Some(text) = fields
        .get("stripped-text")
        .or_else(|| fields.get("body-plain"))
    {
        text.clone()
    } else if let Some(storage_url) = fields.get("storage-url") {
        match fetch_stored_message(&router, &connection_id, storage_url).await {
            Ok(text) => text,
            Err(e) => {
                warn!(error = %e, "Failed to fetch stored Mailgun message");
                return StatusCode::OK.into_response();
            }
        }
    } else {
        warn!(connection_id = %connection_id, "No body or storage URL");
        return StatusCode::OK.into_response();
    };

    if body_plain.trim().is_empty() {
        return StatusCode::OK.into_response();
    }

    // Build the text as "[subject] body" for the userMessage field.
    let text = if subject == "(no subject)" {
        body_plain
    } else {
        format!("[{}] {}", subject, body_plain)
    };

    // Combine file attachments from multipart + metadata from fields.
    let mut attachments = file_attachments;
    attachments.extend(parse_mailgun_attachments(&fields));

    // Upload attachments to internal S3 storage (replaces vendor-specific URLs/data).
    upload_attachments_to_storage(&router, &connection_id, &fields, &mut attachments).await;

    // Build the original message from ALL form fields.
    // Exclude very large fields (body-plain, body-html, stripped-html) to keep
    // originalMessage manageable — they're available via stripped-text/userMessage.
    let exclude_from_original = ["body-plain", "body-html", "stripped-html"];
    let original: serde_json::Map<String, Value> = fields
        .iter()
        .filter(|(k, _)| !exclude_from_original.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();

    let msg = InboundMessage {
        text,
        sender_id: sender_email.clone(),
        conv_id: sender_email.clone(),
        channel: "mailgun".into(),
        attachments,
        original_message: Value::Object(original),
    };

    debug!(
        connection_id = %connection_id,
        sender = %msg.sender_id,
        subject = %subject,
        attachment_count = msg.attachments.len(),
        "Mailgun email processed"
    );

    if let Err(e) = router.handle_message(&connection_id, &msg).await {
        warn!(
            connection_id = %connection_id,
            error = %e,
            "Failed to handle Mailgun email"
        );
    }

    StatusCode::OK.into_response()
}

/// Decode application/x-www-form-urlencoded body into a HashMap.
fn form_decode(body: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in body.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            let key = percent_decode(key);
            let value = percent_decode(value);
            map.insert(key, value);
        }
    }
    map
}

/// Simple percent-decoding for form-urlencoded values.
fn percent_decode(input: &str) -> String {
    let input = input.replace('+', " ");
    let mut result = Vec::with_capacity(input.len());
    let mut bytes = input.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let hi = bytes.next().unwrap_or(0);
            let lo = bytes.next().unwrap_or(0);
            if let (Some(h), Some(l)) = (hex_val(hi), hex_val(lo)) {
                result.push(h << 4 | l);
            } else {
                result.push(b'%');
                result.push(hi);
                result.push(lo);
            }
        } else {
            result.push(b);
        }
    }
    String::from_utf8_lossy(&result).to_string()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Fetch the full email from Mailgun's storage API.
async fn fetch_stored_message(
    router: &ChannelRouter,
    connection_id: &str,
    storage_url: &str,
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
        .ok_or_else(|| anyhow::anyhow!("No parameters"))?;

    let api_key = params["api_key"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing api_key"))?;

    let client = reqwest::Client::new();
    let resp = client
        .get(storage_url)
        .basic_auth("api", Some(api_key))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("HTTP error: {}", e))?;

    let body: Value = resp
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Parse error: {}", e))?;

    let text = body
        .get("stripped-text")
        .or_else(|| body.get("body-plain"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok(text)
}

const ATTACHMENT_BUCKET: &str = "attachments";

/// Upload all attachments to the tenant's internal S3 storage.
///
/// For each attachment:
/// - If `data` is set (multipart base64): decode and upload.
/// - If `url` is set (Mailgun storage URL): download from Mailgun, then upload.
///
/// On success, sets `storage_bucket` and `storage_key` and clears `data`/`url`.
/// On failure, logs a warning and keeps the original attachment as-is.
async fn upload_attachments_to_storage(
    router: &ChannelRouter,
    connection_id: &str,
    _fields: &HashMap<String, String>,
    attachments: &mut [Attachment],
) {
    if attachments.is_empty() {
        return;
    }

    let expected_tenant = crate::config::tenant_id();

    // Resolve S3 client from the tenant's default s3_compatible connection.
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
                    "No S3 storage configured — attachments will not be persisted"
                );
                return;
            }
        };

    // Ensure the attachments bucket exists (idempotent).
    if let Err(e) = s3_client.create_bucket(ATTACHMENT_BUCKET) {
        warn!(error = %e, "Failed to create attachments bucket");
        return;
    }

    // Get Mailgun API key for downloading URL-based attachments.
    let api_key = get_mailgun_api_key(router, connection_id).await;

    let date_prefix = chrono::Utc::now().format("%Y-%m-%d").to_string();

    for attachment in attachments.iter_mut() {
        let sanitized_name = attachment.name.replace(['/', '\\', '\0'], "_");
        let key = format!(
            "{}/{}/{}_{}",
            connection_id,
            date_prefix,
            uuid::Uuid::new_v4().as_simple(),
            sanitized_name
        );

        // Get the binary data — either from base64 inline or by downloading from Mailgun.
        let data = if let Some(b64) = &attachment.data {
            match base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64) {
                Ok(bytes) => bytes,
                Err(e) => {
                    warn!(name = %attachment.name, error = %e, "Failed to decode base64 attachment");
                    continue;
                }
            }
        } else if let Some(url) = &attachment.url {
            match download_mailgun_attachment(router, url, api_key.as_deref()).await {
                Ok(bytes) => bytes,
                Err(e) => {
                    warn!(name = %attachment.name, url = %url, error = %e, "Failed to download Mailgun attachment");
                    continue;
                }
            }
        } else {
            continue;
        };

        // Upload to S3.
        let content_type = Some(attachment.content_type.as_str());
        match s3_client.put_object(ATTACHMENT_BUCKET, &key, data.clone(), content_type) {
            Ok(()) => {
                attachment.size = data.len() as u64;
                attachment.storage_bucket = Some(ATTACHMENT_BUCKET.to_string());
                attachment.storage_key = Some(key.clone());
                attachment.url = None;
                attachment.data = None;
                debug!(name = %attachment.name, storage_key = %key, "Attachment uploaded to internal storage");
            }
            Err(e) => {
                warn!(name = %attachment.name, error = %e, "Failed to upload attachment to S3 — keeping original");
            }
        }
    }
}

/// Get the Mailgun API key from the connection parameters.
async fn get_mailgun_api_key(router: &ChannelRouter, connection_id: &str) -> Option<String> {
    let expected_tenant = crate::config::tenant_id();
    let conn = match router
        .connections()
        .get_with_parameters(connection_id, expected_tenant)
        .await
    {
        Ok(Some(c)) => c,
        Ok(None) => {
            warn!(connection_id = %connection_id, "Connection not found for API key lookup");
            return None;
        }
        Err(e) => {
            warn!(connection_id = %connection_id, error = %e, "DB error looking up connection");
            return None;
        }
    };

    let key = conn
        .connection_parameters
        .as_ref()
        .and_then(|p| p.get("api_key"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    if key.is_none() {
        warn!(connection_id = %connection_id, "No api_key in connection parameters");
    }

    key
}

/// Download an attachment from a Mailgun URL using basic auth.
async fn download_mailgun_attachment(
    router: &ChannelRouter,
    url: &str,
    api_key: Option<&str>,
) -> anyhow::Result<Vec<u8>> {
    let key = api_key.ok_or_else(|| anyhow::anyhow!("No Mailgun API key available"))?;
    let resp = router
        .http_client()
        .get(url)
        .basic_auth("api", Some(key))
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

/// Parse a multipart/form-data request into text fields and file attachments.
async fn parse_multipart(
    request: Request,
) -> Result<(HashMap<String, String>, Vec<Attachment>), String> {
    let mut multipart = Multipart::from_request(request, &())
        .await
        .map_err(|e| format!("Failed to create multipart extractor: {}", e))?;

    let mut fields = HashMap::new();
    let mut attachments = Vec::new();

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| format!("Failed to read multipart field: {}", e))?
    {
        let name = field.name().unwrap_or("unnamed").to_string();
        let file_name = field.file_name().map(|s| s.to_string());
        let content_type = field
            .content_type()
            .map(|s| s.to_string())
            .unwrap_or_default();

        let data = field
            .bytes()
            .await
            .map_err(|e| format!("Failed to read field data: {}", e))?;

        if let Some(filename) = file_name {
            // File attachment — store as base64.
            let base64_data =
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &data);
            attachments.push(Attachment {
                name: filename,
                content_type,
                size: data.len() as u64,
                url: None,
                data: Some(base64_data),
                storage_bucket: None,
                storage_key: None,
            });
        } else {
            // Text field.
            let text_value = String::from_utf8_lossy(&data).to_string();
            fields.insert(name, text_value);
        }
    }

    Ok((fields, attachments))
}

/// Parse attachment metadata from Mailgun form fields.
///
/// Mailgun sends attachment info as JSON in the `attachments` field
/// (when using forward mode) or as part of the stored message.
fn parse_mailgun_attachments(fields: &HashMap<String, String>) -> Vec<Attachment> {
    let mut attachments = Vec::new();

    // Mailgun forward mode: attachments field is a JSON array of objects.
    if let Some(att_json) = fields.get("attachments")
        && let Ok(att_arr) = serde_json::from_str::<Vec<Value>>(att_json)
    {
        for att in att_arr {
            attachments.push(Attachment {
                name: att["name"].as_str().unwrap_or("attachment").to_string(),
                content_type: att["content-type"]
                    .as_str()
                    .unwrap_or("application/octet-stream")
                    .to_string(),
                size: att["size"].as_u64().unwrap_or(0),
                url: att["url"].as_str().map(|s| s.to_string()),
                data: None,
                storage_bucket: None,
                storage_key: None,
            });
        }
    }

    // Also check attachment-count for numbered attachments.
    if let Some(count_str) = fields.get("attachment-count")
        && let Ok(count) = count_str.parse::<usize>()
    {
        for i in 1..=count {
            let key = format!("attachment-{}", i);
            if let Some(info) = fields.get(&key) {
                attachments.push(Attachment {
                    name: info.clone(),
                    content_type: "application/octet-stream".to_string(),
                    size: 0,
                    url: None,
                    data: None,
                    storage_bucket: None,
                    storage_key: None,
                });
            }
        }
    }

    attachments
}

/// Extract email address from "Name <email>" format.
fn extract_email(from: &str) -> String {
    if let Some(start) = from.find('<')
        && let Some(end) = from.find('>')
    {
        return from[start + 1..end].to_string();
    }
    from.to_string()
}

/// Verify the Mailgun webhook signature from form fields.
async fn verify_mailgun_signature(
    router: &ChannelRouter,
    connection_id: &str,
    fields: &HashMap<String, String>,
) -> anyhow::Result<()> {
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

    let signing_key = match params.get("webhook_signing_key").and_then(|v| v.as_str()) {
        Some(key) if !key.is_empty() => key,
        _ => return Ok(()), // No signing key → skip verification.
    };

    let timestamp = fields
        .get("timestamp")
        .ok_or_else(|| anyhow::anyhow!("Missing timestamp"))?;
    let token = fields
        .get("token")
        .ok_or_else(|| anyhow::anyhow!("Missing token"))?;
    let signature = fields
        .get("signature")
        .ok_or_else(|| anyhow::anyhow!("Missing signature"))?;

    let mut mac = Hmac::<Sha256>::new_from_slice(signing_key.as_bytes())
        .map_err(|_| anyhow::anyhow!("Invalid signing key"))?;
    mac.update(timestamp.as_bytes());
    mac.update(token.as_bytes());
    let expected = hex::encode(mac.finalize().into_bytes());

    if !constant_time_eq(signature.as_bytes(), expected.as_bytes()) {
        anyhow::bail!("Signature mismatch");
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
