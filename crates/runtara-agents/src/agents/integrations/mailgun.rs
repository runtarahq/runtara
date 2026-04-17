//! Mailgun Email Operations
//!
//! Send emails via the Mailgun REST API.

use crate::connections::RawConnection;
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::integration_utils::{IntegrationError, ProxyHttpClient, require_connection};

// ============================================================================
// Send Email
// ============================================================================

#[derive(Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Send Email Input")]
pub struct SendEmailInput {
    /// Connection data injected by the workflow runtime
    #[field(skip)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub _connection: Option<RawConnection>,

    #[field(
        display_name = "To",
        description = "Recipient email address(es), comma-separated for multiple",
        example = "user@example.com"
    )]
    pub to: String,

    #[field(
        display_name = "Subject",
        description = "Email subject line",
        example = "Order Confirmation"
    )]
    pub subject: String,

    #[field(display_name = "Text Body", description = "Plain text email body")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,

    #[field(
        display_name = "HTML Body",
        description = "HTML email body (takes precedence over text when both provided)"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,

    #[field(
        display_name = "From",
        description = "Sender email address (defaults to noreply@{domain})"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,

    #[field(display_name = "CC", description = "CC recipients, comma-separated")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc: Option<String>,

    #[field(display_name = "BCC", description = "BCC recipients, comma-separated")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bcc: Option<String>,

    #[field(display_name = "Reply-To", description = "Reply-To email address")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,

    #[field(
        display_name = "Tags",
        description = "Comma-separated tags for tracking"
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<String>,
}

#[derive(Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Send Email Output")]
pub struct SendEmailOutput {
    #[field(
        display_name = "Message ID",
        description = "Mailgun message ID for tracking"
    )]
    pub id: String,

    #[field(display_name = "Message", description = "Mailgun response message")]
    pub message: String,
}

#[capability(
    module = "mailgun",
    display_name = "Send Email (Mailgun)",
    description = "Send an email via Mailgun REST API",
    module_display_name = "Mailgun",
    module_description = "Mailgun email service for sending transactional and marketing emails",
    module_has_side_effects = true,
    module_supports_connections = true,
    module_integration_ids = "mailgun",
    module_secure = true
)]
pub fn send_email(input: SendEmailInput) -> Result<SendEmailOutput, String> {
    let connection = require_connection("MAILGUN", &input._connection)?;

    // `domain` is a non-credential config param needed for path building
    // and the default sender address.
    let domain =
        connection.parameters["domain"]
            .as_str()
            .ok_or(IntegrationError::MissingField {
                prefix: "MAILGUN",
                field: "domain",
                payload: json!({}),
            })?;

    let from = input.from.unwrap_or_else(|| format!("noreply@{}", domain));

    // Build form-urlencoded body.
    let mut form_parts: Vec<(String, String)> = vec![
        ("from".into(), from),
        ("to".into(), input.to),
        ("subject".into(), input.subject),
    ];

    if let Some(text) = input.text {
        form_parts.push(("text".into(), text));
    }
    if let Some(html) = input.html {
        form_parts.push(("html".into(), html));
    }
    if let Some(cc) = input.cc {
        form_parts.push(("cc".into(), cc));
    }
    if let Some(bcc) = input.bcc {
        form_parts.push(("bcc".into(), bcc));
    }
    if let Some(reply_to) = input.reply_to {
        form_parts.push(("h:Reply-To".into(), reply_to));
    }
    if let Some(tags) = input.tags {
        for tag in tags.split(',') {
            form_parts.push(("o:tag".into(), tag.trim().to_string()));
        }
    }

    let client = ProxyHttpClient::new(connection, "MAILGUN");
    let response_json = client
        .post(format!("/v3/{}/messages", domain))
        .form_body(&form_parts)
        .send_json()?;

    Ok(SendEmailOutput {
        id: response_json["id"].as_str().unwrap_or("").to_string(),
        message: response_json["message"]
            .as_str()
            .unwrap_or("Queued")
            .to_string(),
    })
}
