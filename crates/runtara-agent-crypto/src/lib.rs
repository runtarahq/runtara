//! Crypto agent — hashing and HMAC — as a WebAssembly component.
//!
//! Phase 1 minimal implementation: two capabilities (hash, hmac) to validate
//! the end-to-end component pipeline. Full crypto agent (md5, sha1, etc.)
//! follows once the pattern is proven.

#![cfg(target_arch = "wasm32")]

#[allow(warnings)]
mod bindings;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use bindings::exports::runtara::agent::capabilities::{
    CapabilityInfo, ConnectionInfo, ErrorInfo, Guest, ModuleInfo,
};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

struct Component;

impl Guest for Component {
    fn get_module_info() -> ModuleInfo {
        ModuleInfo {
            id: "crypto".into(),
            display_name: "Crypto".into(),
            description: "Hashing and HMAC primitives.".into(),
            has_side_effects: false,
            supports_connections: false,
            integration_ids: vec![],
            secure: false,
        }
    }

    fn list_capabilities() -> Vec<CapabilityInfo> {
        vec![
            CapabilityInfo {
                id: "hash".into(),
                function_name: "hash".into(),
                display_name: Some("Hash".into()),
                description: Some("Compute a SHA-256 hash of a UTF-8 string.".into()),
                has_side_effects: false,
                is_idempotent: true,
                rate_limited: false,
                tags: vec!["crypto".into()],
                input_schema: HASH_INPUT_SCHEMA.into(),
                output_schema: HASH_OUTPUT_SCHEMA.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
            CapabilityInfo {
                id: "hmac".into(),
                function_name: "hmac".into(),
                display_name: Some("HMAC".into()),
                description: Some("Compute an HMAC-SHA-256 over a UTF-8 string.".into()),
                has_side_effects: false,
                is_idempotent: true,
                rate_limited: false,
                tags: vec!["crypto".into()],
                input_schema: HMAC_INPUT_SCHEMA.into(),
                output_schema: HMAC_OUTPUT_SCHEMA.into(),
                known_errors: vec![],
                compensation_hint: None,
            },
        ]
    }

    fn invoke(
        capability_id: String,
        input: String,
        _connection: Option<ConnectionInfo>,
    ) -> Result<String, ErrorInfo> {
        match capability_id.as_str() {
            "hash" => hash(&input),
            "hmac" => hmac_sha256(&input),
            other => Err(ErrorInfo {
                code: "UNKNOWN_CAPABILITY".into(),
                message: format!("crypto agent has no capability `{other}`"),
                category: "permanent".into(),
                severity: "error".into(),
                retryable: false,
                retry_after_ms: None,
                attributes: None,
            }),
        }
    }
}

fn hash(input_json: &str) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct In {
        value: String,
    }
    #[derive(serde::Serialize)]
    struct Out {
        hex: String,
        base64: String,
    }

    let input: In = serde_json::from_str(input_json).map_err(json_err)?;
    let digest = Sha256::digest(input.value.as_bytes());
    let out = Out {
        hex: hex_encode(&digest),
        base64: BASE64.encode(digest),
    };
    serde_json::to_string(&out).map_err(json_err)
}

fn hmac_sha256(input_json: &str) -> Result<String, ErrorInfo> {
    #[derive(serde::Deserialize)]
    struct In {
        key: String,
        message: String,
    }
    #[derive(serde::Serialize)]
    struct Out {
        hex: String,
        base64: String,
    }

    let input: In = serde_json::from_str(input_json).map_err(json_err)?;
    let mut mac = Hmac::<Sha256>::new_from_slice(input.key.as_bytes()).map_err(|e| ErrorInfo {
        code: "INVALID_KEY".into(),
        message: e.to_string(),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
    })?;
    mac.update(input.message.as_bytes());
    let tag = mac.finalize().into_bytes();
    let out = Out {
        hex: hex_encode(&tag),
        base64: BASE64.encode(tag),
    };
    serde_json::to_string(&out).map_err(json_err)
}

fn json_err(e: serde_json::Error) -> ErrorInfo {
    ErrorInfo {
        code: "BAD_JSON".into(),
        message: e.to_string(),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

const HASH_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["value"],
    "properties": { "value": { "type": "string" } }
}"#;

const HASH_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "hex":    { "type": "string" },
        "base64": { "type": "string" }
    }
}"#;

const HMAC_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["key", "message"],
    "properties": {
        "key":     { "type": "string" },
        "message": { "type": "string" }
    }
}"#;

const HMAC_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "hex":    { "type": "string" },
        "base64": { "type": "string" }
    }
}"#;

bindings::export!(Component with_types_in bindings);
