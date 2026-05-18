//! XLSX agent — thin wrapper component.
//!
//! Excel-spreadsheet parsing uses calamine + native zip; thinly wraps a call
//! to `$RUNTARA_AGENT_SERVICE_URL/{module}/{capability}` where the host's
//! native binary handles the work. See `docs/wasm-components-migration-plan.md
//! § 5.6`.

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
            id: "xlsx".into(),
            display_name: "XLSX".into(),
            description:
                "Excel-spreadsheet parsing. Runs on the host via the native agent service.".into(),
            has_side_effects: false,
            supports_connections: false,
            integration_ids: vec![],
            secure: false,
        }
    }

    fn list_capabilities() -> Vec<CapabilityInfo> {
        vec![
            cap(
                "from-xlsx",
                "from_xlsx",
                "Parse XLSX",
                "Parse an XLSX file into a JSON array of rows.",
                FROM_INPUT,
                FROM_OUTPUT,
            ),
            cap(
                "get-sheets",
                "get_sheets",
                "List Sheets",
                "List sheet names and dimensions in an XLSX file.",
                SHEETS_INPUT,
                SHEETS_OUTPUT,
            ),
        ]
    }

    fn invoke(
        capability_id: String,
        input: String,
        connection: Option<ConnectionInfo>,
    ) -> Result<String, ErrorInfo> {
        forward_to_native("xlsx", &capability_id, &input, connection.as_ref())
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
        tags: vec!["xlsx".into(), "spreadsheet".into(), "native".into()],
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

const FROM_INPUT: &str = r#"{"type":"object","required":["data"],"properties":{"data":{"description":"FileData or base64 string"},"sheet_name":{"type":"string"},"has_headers":{"type":"boolean","default":true}}}"#;
const FROM_OUTPUT: &str = r#"{"type":"array","description":"Rows as objects or arrays"}"#;
const SHEETS_INPUT: &str = r#"{"type":"object","required":["data"],"properties":{"data":{"description":"FileData or base64 string"}}}"#;
const SHEETS_OUTPUT: &str = r#"{"type":"array","items":{"type":"object","properties":{"name":{"type":"string"},"rows":{"type":"integer"},"cols":{"type":"integer"}}}}"#;

bindings::export!(Component with_types_in bindings);
