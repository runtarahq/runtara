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

    // Parse without fetching any provider content. Preserve the provider payload
    // separately from the text projection used for verification and routing.
    let (fields, file_attachments, original) = if content_type.starts_with("multipart/form-data") {
        match parse_multipart(Request::from_parts(parts, body)).await {
            Ok((fields, attachments)) => {
                let original = serde_json::to_value(&fields).unwrap_or_default();
                (fields, attachments, original)
            }
            Err(e) => {
                warn!(error = %e, "Failed to parse multipart");
                return StatusCode::BAD_REQUEST.into_response();
            }
        }
    } else {
        let bytes = match axum::body::to_bytes(
            body,
            crate::api::handlers::events::webhook_max_body_bytes(),
        )
        .await
        {
            Ok(bytes) => bytes,
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        };
        let original = if content_type.starts_with("application/json") {
            match serde_json::from_slice::<Value>(&bytes) {
                Ok(value) if value.is_object() => value,
                _ => return StatusCode::BAD_REQUEST.into_response(),
            }
        } else {
            serde_json::from_slice::<Value>(&bytes)
                .ok()
                .filter(Value::is_object)
                .unwrap_or_else(|| {
                    serde_json::to_value(form_decode(&String::from_utf8_lossy(&bytes)))
                        .unwrap_or_default()
                })
        };
        (mailgun_fields(&original), Vec::new(), original)
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

    let Some(msg) = normalize_mailgun_message(&fields, file_attachments, original) else {
        return StatusCode::OK.into_response();
    };

    debug!(
        connection_id = %connection_id,
        sender = %msg.sender_id,
        attachment_count = msg.attachments.len(),
        "Mailgun email processed"
    );

    // Stored before the 200; a redelivered Message-Id is acknowledged and dropped.
    router
        .receive(&connection_id, msg, false)
        .await
        .into_response()
}

/// Keep nested JSON in originalMessage; project only fields needed for routing,
/// signature verification, and normalized attachment metadata.
fn mailgun_fields(original: &Value) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    if let Some(object) = original.as_object() {
        for (key, value) in object {
            if let Some(value) = value.as_str() {
                fields.insert(key.clone(), value.to_string());
            } else if value.is_number() || (key == "attachments" && value.is_array()) {
                fields.insert(key.clone(), value.to_string());
            }
        }
    }
    for (key, pointer) in [
        ("sender", "/envelope/sender"),
        ("from", "/message/headers/from"),
        ("subject", "/message/headers/subject"),
        ("Message-Id", "/message/headers/message-id"),
        ("timestamp", "/signature/timestamp"),
        ("token", "/signature/token"),
        ("signature", "/signature/signature"),
    ] {
        if let Some(value) = original.pointer(pointer) {
            if let Some(text) = value.as_str() {
                fields.entry(key.into()).or_insert(text.into());
            } else if value.is_number() {
                fields.entry(key.into()).or_insert(value.to_string());
            }
        }
    }
    if let Some(url) = original
        .pointer("/storage/url")
        .and_then(|url| url.as_str().or_else(|| url.as_array()?.first()?.as_str()))
    {
        fields.entry("storage-url".into()).or_insert(url.into());
    }
    fields
}

fn normalize_mailgun_message(
    fields: &HashMap<String, String>,
    mut attachments: Vec<Attachment>,
    original: Value,
) -> Option<InboundMessage> {
    let sender = fields.get("sender").or_else(|| fields.get("from"))?;
    let sender_email = extract_email(sender);
    attachments.extend(parse_mailgun_attachments(fields));
    let body = fields
        .get("stripped-text")
        .or_else(|| fields.get("body-plain"))
        .map(String::as_str)
        .unwrap_or("");
    let has_stored_message = ["message-url", "storage-url"]
        .iter()
        .any(|key| fields.get(*key).is_some_and(|value| !value.is_empty()));
    if body.trim().is_empty()
        && attachments.is_empty()
        && !has_stored_message
        && !fields.contains_key("body-html")
    {
        return None;
    }
    let text = if body.trim().is_empty() {
        String::new()
    } else if let Some(subject) = fields.get("subject") {
        format!("[{subject}] {body}")
    } else {
        body.to_string()
    };
    Some(InboundMessage {
        text,
        sender_id: sender_email.clone(),
        conv_id: sender_email,
        channel: "mailgun".into(),
        attachments,
        original_message: original,
        target: None,
        activity_id: fields
            .get("Message-Id")
            .or_else(|| fields.get("token"))
            .cloned(),
        intake_id: None,
        workflow: None,
    })
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
                id: None,
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
                id: None,
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
                    id: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stored_notifications_need_no_body_and_preserve_nested_json() {
        for original in [
            json!({"sender": "sender@example.test", "message-url": "https://storage-us-west1.api.mailgun.net/message"}),
            json!({"sender": "sender@example.test", "storage-url": "https://storage-us-west1.api.mailgun.net/message"}),
            json!({"envelope": {"sender": "sender@example.test"}, "storage": {"url": ["https://storage-us-west1.api.mailgun.net/message"]}, "message": {"headers": {"message-id": "msg-1"}}, "unknown": {"preserved": [1,2,3]}}),
        ] {
            let message =
                normalize_mailgun_message(&mailgun_fields(&original), vec![], original.clone())
                    .unwrap();
            assert!(message.text.is_empty());
            assert_eq!(message.original_message, original);
            assert!(message.attachments.is_empty());
        }
    }

    #[test]
    fn email_bodies_and_attachment_references_reach_workflow_unchanged() {
        let original = json!({"sender": "sender@example.test", "body-plain": "body", "body-html": "<p>body</p>", "attachments": [{"name":"invoice.pdf", "url":"https://storage-us-west1.api.mailgun.net/file", "size": 20}]});
        let message =
            normalize_mailgun_message(&mailgun_fields(&original), vec![], original.clone())
                .unwrap();
        assert_eq!(message.original_message, original);
        assert_eq!(
            message.attachments[0].url.as_deref(),
            Some("https://storage-us-west1.api.mailgun.net/file")
        );
        assert!(message.attachments[0].data.is_none());
    }

    #[tokio::test]
    async fn multipart_file_only_message_keeps_received_bytes() {
        let body = "--boundary\r\nContent-Disposition: form-data; name=\"sender\"\r\n\r\nsender@example.test\r\n--boundary\r\nContent-Disposition: form-data; name=\"attachment-1\"; filename=\"note.txt\"\r\nContent-Type: text/plain\r\n\r\nhello\r\n--boundary--\r\n";
        let request = Request::builder()
            .header("content-type", "multipart/form-data; boundary=boundary")
            .body(axum::body::Body::from(body))
            .unwrap();
        let (fields, attachments) = parse_multipart(request).await.unwrap();
        let original = serde_json::to_value(&fields).unwrap();
        let message = normalize_mailgun_message(&fields, attachments, original).unwrap();
        assert!(message.text.is_empty());
        assert_eq!(message.attachments.len(), 1);
        assert_eq!(message.attachments[0].data.as_deref(), Some("aGVsbG8="));
        assert_eq!(message.attachments[0].name, "note.txt");
        assert!(message.attachments[0].url.is_none());
    }
}
