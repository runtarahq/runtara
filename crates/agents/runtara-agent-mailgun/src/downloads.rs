use super::*;
use base64::Engine as _;

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get Message Input")]
pub struct GetMessageInput {
    #[field(skip)]
    #[serde(default)]
    pub _connection: Option<RawConnection>,
    #[field(
        display_name = "Message URL",
        description = "Mailgun message-url or storage.url from the incoming event"
    )]
    pub url: String,
    #[field(
        display_name = "Maximum Bytes",
        description = "Maximum response size, from 1 to 5242880 bytes (default 5 MiB)"
    )]
    #[serde(default)]
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get Message Output")]
pub struct GetMessageOutput {
    #[field(
        display_name = "Message",
        description = "Complete Mailgun stored-message JSON, including text and HTML bodies"
    )]
    pub message: Value,
    #[field(
        display_name = "Attachments",
        description = "Attachment metadata and URLs; content is only fetched by Download Attachment"
    )]
    pub attachments: Vec<Value>,
}

#[capability(
    module = "mailgun",
    display_name = "Get Message",
    description = "Retrieve a stored Mailgun email. Returns email fields and attachment references without downloading attachments.",
    side_effects = false,
    rate_limited = true
)]
pub fn get_message(input: GetMessageInput) -> Result<GetMessageOutput, AgentError> {
    let response = fetch(
        input._connection.as_ref(),
        &input.url,
        "application/json",
        input.max_bytes,
    )?;
    parse_message(&response.body)
}

fn parse_message(body: &[u8]) -> Result<GetMessageOutput, AgentError> {
    let message: Value = serde_json::from_slice(body).map_err(|_| invalid_message())?;
    if !message.is_object() {
        return Err(invalid_message());
    }
    let attachments = match message.get("attachments") {
        Some(Value::Array(values)) => values.clone(),
        Some(Value::String(value)) => {
            serde_json::from_str::<Vec<Value>>(value).map_err(|_| invalid_message())?
        }
        None | Some(Value::Null) => vec![],
        _ => return Err(invalid_message()),
    };
    Ok(GetMessageOutput {
        message,
        attachments,
    })
}

fn invalid_message() -> AgentError {
    AgentError::permanent(
        "MAILGUN_INVALID_MESSAGE",
        "Mailgun returned an invalid stored-message JSON response",
    )
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Download Attachment Input")]
pub struct DownloadAttachmentInput {
    #[field(skip)]
    #[serde(default)]
    pub _connection: Option<RawConnection>,
    #[field(
        display_name = "Attachment URL",
        description = "Attachment URL from the webhook or Get Message result"
    )]
    pub url: String,
    #[field(
        display_name = "Filename",
        description = "Original filename from attachment metadata"
    )]
    #[serde(default)]
    pub filename: Option<String>,
    #[field(
        display_name = "Maximum Bytes",
        description = "Maximum download size, from 1 to 5242880 bytes (default 5 MiB)"
    )]
    #[serde(default)]
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Download Attachment Output")]
pub struct DownloadAttachmentOutput {
    #[field(
        display_name = "Content",
        description = "Base64-encoded attachment content"
    )]
    pub content: String,
    #[field(display_name = "Filename")]
    pub filename: String,
    #[field(display_name = "Content Type")]
    pub content_type: String,
    #[field(display_name = "Size", description = "Actual size in bytes")]
    pub size: u64,
}

#[capability(
    module = "mailgun",
    display_name = "Download Attachment",
    description = "Download one Mailgun attachment using the selected connection. Returns base64 content; never stores it automatically.",
    side_effects = false,
    rate_limited = true
)]
pub fn download_attachment(
    input: DownloadAttachmentInput,
) -> Result<DownloadAttachmentOutput, AgentError> {
    let response = fetch(
        input._connection.as_ref(),
        &input.url,
        "application/octet-stream",
        input.max_bytes,
    )?;
    Ok(DownloadAttachmentOutput {
        filename: input.filename.unwrap_or_else(|| "attachment".into()),
        content_type: response
            .header("content-type")
            .unwrap_or("application/octet-stream")
            .into(),
        content: base64::engine::general_purpose::STANDARD.encode(&response.body),
        size: response.body.len() as u64,
    })
}

fn fetch(
    connection: Option<&RawConnection>,
    url: &str,
    accept: &str,
    max_bytes: Option<usize>,
) -> Result<runtara_http::HttpResponse, AgentError> {
    let limit = runtara_http::download::byte_limit(max_bytes).map_err(download_error)?;
    let connection = connection.ok_or_else(|| {
        AgentError::permanent("MAILGUN_MISSING_CONNECTION", "Select a Mailgun connection")
    })?;
    let endpoint = storage_endpoint(url)?;
    runtara_http::download::get(
        &connection.connection_id,
        url,
        Some(endpoint),
        accept,
        limit,
    )
    .map_err(download_error)
}

fn storage_endpoint(url: &str) -> Result<&'static str, AgentError> {
    let host = url
        .strip_prefix("https://")
        .and_then(|rest| rest.split_once('/'))
        .map(|(host, _)| host);
    match host {
        Some("api.mailgun.net") => Ok("messages-us"),
        Some("api.eu.mailgun.net") => Ok("messages-eu"),
        Some("storage-us-west1.api.mailgun.net") => Ok("storage-us-west1"),
        Some("storage-us-east4.api.mailgun.net") => Ok("storage-us-east4"),
        Some("storage-europe-west1.api.mailgun.net") => Ok("storage-europe-west1"),
        Some("storage.api.mailgun.net") => Ok("storage-us"),
        Some("storage.eu.mailgun.net") => Ok("storage-eu"),
        _ => Err(AgentError::permanent(
            "MAILGUN_INVALID_STORAGE_URL",
            "Expected an approved Mailgun HTTPS storage URL",
        )),
    }
}

fn download_error(error: runtara_http::download::DownloadError) -> AgentError {
    let mut result = if error.transient {
        AgentError::transient(error.code, error.message)
    } else {
        AgentError::permanent(error.code, error.message)
    };
    result.retry_after_ms = error.retry_after_ms;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_preserves_bodies_and_normalizes_attachment_metadata() {
        for attachments in [
            serde_json::json!([{"name": "invoice.pdf", "url": "https://storage-us-west1.api.mailgun.net/attachment"}]),
            serde_json::json!("[{\"name\":\"invoice.pdf\"}]"),
        ] {
            let result = parse_message(&serde_json::to_vec(&serde_json::json!({"body-plain": "hello", "body-html": "<p>hello</p>", "attachments": attachments})).unwrap()).unwrap();
            assert_eq!(result.message["body-html"], "<p>hello</p>");
            assert_eq!(result.attachments[0]["name"], "invoice.pdf");
        }
        assert!(parse_message(b"not JSON").is_err());
        assert!(parse_message(br#"{"attachments":"broken"}"#).is_err());
    }

    #[test]
    fn storage_urls_select_only_declared_hosts() {
        assert_eq!(
            storage_endpoint(
                "https://storage-us-east4.api.mailgun.net/v3/domains/example/messages/key"
            )
            .unwrap(),
            "storage-us-east4"
        );
        for url in [
            "http://api.mailgun.net/a",
            "https://api.mailgun.net.evil.test/a",
            "https://api.mailgun.net@evil.test/a",
            "https://localhost/a",
            "https://api.mailgun.net:444/a",
        ] {
            assert!(storage_endpoint(url).is_err());
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub(super) fn execute_get_message(input: Value) -> Result<Value, String> {
    __executor_get_message(input)
}

#[cfg(target_arch = "wasm32")]
pub(super) fn execute_download_attachment(input: Value) -> Result<Value, String> {
    __executor_download_attachment(input)
}
