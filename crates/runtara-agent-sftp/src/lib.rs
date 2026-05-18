//! SFTP agent — thin wrapper component.
//!
//! SFTP requires libssh2 (C lib) which doesn't compile to wasm32-wasip2. This
//! component forwards every invoke() to the host's existing internal native
//! agent endpoint at `$RUNTARA_AGENT_SERVICE_URL/{module}/{capability}` where
//! the native binary owns the C-deps. Pattern documented in
//! `docs/wasm-components-migration-plan.md § 5.6` and matches today's
//! `runtara-workflow-stdlib::dispatch::native_agent_stub`.

#![cfg(target_arch = "wasm32")]

#[allow(warnings)]
mod bindings;

use bindings::exports::runtara::agent::capabilities::{
    CapabilityInfo, ConnectionInfo, ErrorInfo, Guest, ModuleInfo,
};
use serde_json::Value;
use std::time::Duration;

struct Component;

impl Guest for Component {
    fn get_module_info() -> ModuleInfo {
        ModuleInfo {
            id: "sftp".into(),
            display_name: "SFTP".into(),
            description: "SFTP file operations (list, upload, download, delete). \
                          Runs on the host via the native agent service."
                .into(),
            has_side_effects: true,
            supports_connections: true,
            integration_ids: vec!["sftp".into()],
            secure: true,
        }
    }

    fn list_capabilities() -> Vec<CapabilityInfo> {
        vec![
            cap(
                "sftp-list-files",
                "sftp_list_files",
                "SFTP List Files",
                "List files in a directory on the remote SFTP server.",
                LIST_INPUT,
                LIST_OUTPUT,
            ),
            cap(
                "sftp-download-file",
                "sftp_download_file",
                "SFTP Download File",
                "Download a file from the remote SFTP server (returns base64).",
                DOWNLOAD_INPUT,
                DOWNLOAD_OUTPUT,
            ),
            cap(
                "sftp-upload-file",
                "sftp_upload_file",
                "SFTP Upload File",
                "Upload base64-encoded content to a path on the remote SFTP server.",
                UPLOAD_INPUT,
                UPLOAD_OUTPUT,
            ),
            cap(
                "sftp-delete-file",
                "sftp_delete_file",
                "SFTP Delete File",
                "Delete a file on the remote SFTP server.",
                DELETE_INPUT,
                DELETE_OUTPUT,
            ),
        ]
    }

    fn invoke(
        capability_id: String,
        input: String,
        connection: Option<ConnectionInfo>,
    ) -> Result<String, ErrorInfo> {
        forward_to_native("sftp", &capability_id, &input, connection.as_ref())
    }
}

// -----------------------------------------------------------------------------
// Native-stub forwarder (shared shape for all wrappers).
// -----------------------------------------------------------------------------

fn forward_to_native(
    module: &str,
    capability_id: &str,
    input_json: &str,
    connection: Option<&ConnectionInfo>,
) -> Result<String, ErrorInfo> {
    let base = std::env::var("RUNTARA_AGENT_SERVICE_URL").map_err(|_| {
        permanent_err(
            "AGENT_SERVICE_URL_MISSING",
            "RUNTARA_AGENT_SERVICE_URL not set; native wrapper cannot forward",
        )
    })?;
    let url = format!("{}/{module}/{capability_id}", base.trim_end_matches('/'));

    // Re-inject _connection into the input JSON (matches today's
    // native_agent_stub envelope shape — the native side expects
    // _connection embedded in the input object).
    let mut input_value: Value = serde_json::from_str(input_json)
        .map_err(|e| permanent_err("INPUT_DESERIALIZATION_ERROR", e.to_string()))?;
    if let Some(conn) = connection
        && let Value::Object(ref mut map) = input_value
    {
        map.insert(
            "_connection".into(),
            serde_json::json!({
                "connection_id": conn.connection_id,
                "integration_id": conn.integration_id,
                "connection_subtype": conn.connection_subtype,
                "parameters": serde_json::from_str::<Value>(&conn.parameters)
                    .unwrap_or(Value::Object(Default::default())),
                "rate_limit_config": conn.rate_limit_config.as_ref().and_then(|s| {
                    serde_json::from_str::<Value>(s).ok()
                }),
            }),
        );
    }
    let body = serde_json::to_vec(&input_value)
        .map_err(|e| permanent_err("INPUT_SERIALIZATION_ERROR", e.to_string()))?;

    let tenant_id = std::env::var("RUNTARA_TENANT_ID").unwrap_or_default();

    let client = runtara_http::HttpClient::with_timeout(Duration::from_secs(120));
    let response = client
        .request("POST", &url)
        .header("Content-Type", "application/json")
        .header("X-Org-Id", &tenant_id)
        .body_bytes(&body)
        .call()
        .map_err(|e| transient_err("NETWORK_ERROR", format!("native agent call failed: {e}")))?;

    let status = response.status;
    let body_text = String::from_utf8_lossy(&response.body).to_string();
    if !(200..300).contains(&status) {
        return Err(permanent_err(
            format!("NATIVE_AGENT_HTTP_{status}").as_str(),
            format!("native agent {module}/{capability_id} returned {status}: {body_text}"),
        ));
    }
    Ok(body_text)
}

fn cap(
    id: &str,
    function_name: &str,
    display_name: &str,
    description: &str,
    input_schema: &str,
    output_schema: &str,
) -> CapabilityInfo {
    CapabilityInfo {
        id: id.into(),
        function_name: function_name.into(),
        display_name: Some(display_name.into()),
        description: Some(description.into()),
        has_side_effects: true,
        is_idempotent: false,
        rate_limited: false,
        tags: vec!["sftp".into(), "io".into(), "native".into()],
        input_schema: input_schema.into(),
        output_schema: output_schema.into(),
        known_errors: vec![],
        compensation_hint: None,
    }
}

fn permanent_err(code: &str, message: impl Into<String>) -> ErrorInfo {
    ErrorInfo {
        code: code.into(),
        message: message.into(),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
    }
}

fn transient_err(code: &str, message: impl Into<String>) -> ErrorInfo {
    ErrorInfo {
        code: code.into(),
        message: message.into(),
        category: "transient".into(),
        severity: "warning".into(),
        retryable: true,
        retry_after_ms: None,
        attributes: None,
    }
}

const LIST_INPUT: &str =
    r#"{"type":"object","required":["path"],"properties":{"path":{"type":"string"}}}"#;
const LIST_OUTPUT: &str = r#"{"type":"array","items":{"type":"object","properties":{"name":{"type":"string"},"size":{"type":"integer"},"modified":{"type":"string"},"is_dir":{"type":"boolean"}}}}"#;

const DOWNLOAD_INPUT: &str =
    r#"{"type":"object","required":["path"],"properties":{"path":{"type":"string"}}}"#;
const DOWNLOAD_OUTPUT: &str = r#"{"type":"string","description":"Base64-encoded file content"}"#;

const UPLOAD_INPUT: &str = r#"{"type":"object","required":["path","content"],"properties":{"path":{"type":"string"},"content":{"type":"string","description":"Base64-encoded content"}}}"#;
const UPLOAD_OUTPUT: &str = r#"{"type":"integer","description":"Bytes written"}"#;

const DELETE_INPUT: &str =
    r#"{"type":"object","required":["path"],"properties":{"path":{"type":"string"}}}"#;
const DELETE_OUTPUT: &str =
    r#"{"type":"object","properties":{"deleted":{"type":"boolean"},"path":{"type":"string"}}}"#;

bindings::export!(Component with_types_in bindings);
