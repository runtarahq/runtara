//! Compression agent — thin wrapper component.
//!
//! Zip/unzip operations require native zip libs that don't compile to
//! wasm32-wasip2 cleanly. This component forwards every invoke() to the host's
//! existing internal native agent endpoint at
//! `$RUNTARA_AGENT_SERVICE_URL/{module}/{capability}`. See
//! `docs/wasm-components-migration-plan.md § 5.6`.

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
            id: "compression".into(),
            display_name: "Compression".into(),
            description: "ZIP archive create/extract/list operations. \
                          Runs on the host via the native agent service."
                .into(),
            has_side_effects: false,
            supports_connections: false,
            integration_ids: vec![],
            secure: false,
        }
    }

    fn list_capabilities() -> Vec<CapabilityInfo> {
        vec![
            cap(
                "create-archive",
                "create_archive",
                "Create Archive",
                "Build a ZIP archive from one or more base64-encoded files.",
                ARCHIVE_INPUT,
                FILE_OUTPUT,
            ),
            cap(
                "extract-archive",
                "extract_archive",
                "Extract Archive",
                "Extract every entry from a ZIP archive.",
                EXTRACT_INPUT,
                EXTRACT_OUTPUT,
            ),
            cap(
                "extract-file",
                "extract_file",
                "Extract File",
                "Extract a single named entry from a ZIP archive.",
                EXTRACT_FILE_INPUT,
                FILE_OUTPUT,
            ),
            cap(
                "list-archive",
                "list_archive",
                "List Archive",
                "List entries inside a ZIP archive without extracting.",
                LIST_INPUT,
                LIST_OUTPUT,
            ),
        ]
    }

    fn invoke(
        capability_id: String,
        input: String,
        connection: Option<ConnectionInfo>,
    ) -> Result<String, ErrorInfo> {
        forward_to_native("compression", &capability_id, &input, connection.as_ref())
    }
}

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

    let response = runtara_http::HttpClient::with_timeout(Duration::from_secs(120))
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
        has_side_effects: false,
        is_idempotent: true,
        rate_limited: false,
        tags: vec!["compression".into(), "zip".into(), "native".into()],
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

const ARCHIVE_INPUT: &str = r#"{"type":"object","required":["files"],"properties":{"files":{"type":"array"},"compression_level":{"type":"integer"}}}"#;
const FILE_OUTPUT: &str = r#"{"type":"object","properties":{"content":{"type":"string"},"filename":{"type":"string"},"mime_type":{"type":"string"}}}"#;
const EXTRACT_INPUT: &str = r#"{"type":"object","required":["archive"],"properties":{"archive":{"description":"FileData or base64 string"}}}"#;
const EXTRACT_OUTPUT: &str = r#"{"type":"object","properties":{"files":{"type":"array"}}}"#;
const EXTRACT_FILE_INPUT: &str = r#"{"type":"object","required":["archive","path"],"properties":{"archive":{"description":"FileData or base64 string"},"path":{"type":"string"}}}"#;
const LIST_INPUT: &str = r#"{"type":"object","required":["archive"],"properties":{"archive":{"description":"FileData or base64 string"}}}"#;
const LIST_OUTPUT: &str = r#"{"type":"object","properties":{"entries":{"type":"array"}}}"#;

bindings::export!(Component with_types_in bindings);
