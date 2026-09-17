use super::*;
use base64::Engine as _;

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Get File Info Input")]
pub struct GetFileInfoInput {
    #[field(skip)]
    #[serde(default)]
    pub _connection: Option<RawConnection>,
    #[field(
        display_name = "File ID",
        description = "Slack file ID from the inbound event"
    )]
    pub file_id: String,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Get File Info Output")]
pub struct GetFileInfoOutput {
    #[field(
        display_name = "File",
        description = "Slack file metadata, including size, MIME type and private download URLs; no file content"
    )]
    pub file: Value,
}

#[capability(
    module = "slack",
    display_name = "Get File Info",
    description = "Read Slack file metadata without downloading content. Requires files:read.",
    side_effects = false,
    rate_limited = true
)]
pub fn get_file_info(input: GetFileInfoInput) -> Result<GetFileInfoOutput, AgentError> {
    let connection = input._connection.as_ref().ok_or_else(missing_connection)?;
    if input.file_id.is_empty() {
        return Err(AgentError::permanent(
            "SLACK_INVALID_FILE_ID",
            "file_id must not be empty",
        ));
    }
    let response = slack_api_call("files.info", connection, &json!({"file": input.file_id}))?;
    let file = response
        .get("file")
        .filter(|file| file.is_object())
        .cloned()
        .ok_or_else(|| {
            AgentError::permanent(
                "SLACK_INVALID_FILE_RESPONSE",
                "Slack returned no file metadata",
            )
        })?;
    Ok(GetFileInfoOutput { file })
}

#[derive(Debug, Serialize, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Download File Input")]
pub struct DownloadFileInput {
    #[field(skip)]
    #[serde(default)]
    pub _connection: Option<RawConnection>,
    #[field(
        display_name = "File ID",
        description = "Slack file ID from the inbound event"
    )]
    pub file_id: String,
    #[field(
        display_name = "Maximum Bytes",
        description = "Maximum download size, from 1 to 5242880 bytes (default 5 MiB)"
    )]
    #[serde(default)]
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(display_name = "Download File Output")]
pub struct DownloadFileOutput {
    #[field(display_name = "Content", description = "Base64-encoded file content")]
    pub content: String,
    #[field(display_name = "Filename")]
    pub filename: String,
    #[field(display_name = "Content Type")]
    pub content_type: String,
    #[field(display_name = "Size", description = "Actual size in bytes")]
    pub size: u64,
}

#[capability(
    module = "slack",
    display_name = "Download File",
    description = "Download a Slack-hosted file by ID with files:read. Returns base64 content; never stores it automatically.",
    side_effects = false,
    rate_limited = true
)]
pub fn download_file(input: DownloadFileInput) -> Result<DownloadFileOutput, AgentError> {
    let limit = runtara_http::download::byte_limit(input.max_bytes).map_err(download_error)?;
    let connection = input._connection.as_ref().ok_or_else(missing_connection)?;
    let file = get_file_info(GetFileInfoInput {
        _connection: Some(connection.clone()),
        file_id: input.file_id,
    })?
    .file;
    if file
        .get("size")
        .and_then(Value::as_u64)
        .is_some_and(|size| size > limit as u64)
    {
        return Err(AgentError::permanent(
            "DOWNLOAD_TOO_LARGE",
            "Slack file exceeds max_bytes",
        ));
    }
    let url = file
        .get("url_private_download")
        .and_then(Value::as_str)
        .filter(|url| !url.is_empty())
        .or_else(|| file.get("url_private").and_then(Value::as_str))
        .ok_or_else(|| {
            AgentError::permanent(
                "SLACK_FILE_UNAVAILABLE",
                "Slack file has no downloadable content",
            )
        })?;
    let endpoint = file_endpoint(url)?;
    let response = runtara_http::download::get(
        &connection.connection_id,
        url,
        endpoint,
        "application/octet-stream",
        limit,
    )
    .map_err(download_error)?;
    Ok(DownloadFileOutput {
        content: base64::engine::general_purpose::STANDARD.encode(&response.body),
        filename: file
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("attachment")
            .into(),
        content_type: file
            .get("mimetype")
            .and_then(Value::as_str)
            .or_else(|| response.header("content-type"))
            .unwrap_or("application/octet-stream")
            .into(),
        size: response.body.len() as u64,
    })
}

fn file_endpoint(url: &str) -> Result<Option<&'static str>, AgentError> {
    if url.starts_with("https://files.slack.com/files-pri/") {
        Ok(Some("files"))
    } else if url.starts_with("https://slack.com/files-pri/") {
        Ok(None)
    } else {
        Err(AgentError::permanent(
            "SLACK_INVALID_FILE_URL",
            "Expected a Slack private file URL",
        ))
    }
}

fn missing_connection() -> AgentError {
    AgentError::permanent("SLACK_MISSING_CONNECTION", "Select a Slack connection")
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
    fn private_file_urls_only() {
        assert_eq!(
            file_endpoint("https://files.slack.com/files-pri/T-F/file.pdf").unwrap(),
            Some("files")
        );
        for url in [
            "https://files.slack.com.evil.test/files-pri/file",
            "http://files.slack.com/files-pri/file",
            "https://files.slack.com@evil.test/files-pri/file",
            "https://evil.test/file",
            "https://files.slack.com/upload/file",
        ] {
            assert!(file_endpoint(url).is_err());
        }
    }

    #[test]
    fn download_requires_connection_and_valid_limit_before_network() {
        let error = download_file(DownloadFileInput {
            _connection: None,
            file_id: "F123".into(),
            max_bytes: None,
        })
        .unwrap_err();
        assert_eq!(error.code, "SLACK_MISSING_CONNECTION");
        let error = download_file(DownloadFileInput {
            _connection: None,
            file_id: "F123".into(),
            max_bytes: Some(0),
        })
        .unwrap_err();
        assert_eq!(error.code, "INVALID_DOWNLOAD_LIMIT");
    }
}

#[cfg(target_arch = "wasm32")]
pub(super) fn execute_get_file_info(input: Value) -> Result<Value, String> {
    __executor_get_file_info(input)
}

#[cfg(target_arch = "wasm32")]
pub(super) fn execute_download_file(input: Value) -> Result<Value, String> {
    __executor_download_file(input)
}
