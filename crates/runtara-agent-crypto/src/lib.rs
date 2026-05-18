//! Crypto agent — hashing and HMAC — as a WebAssembly component.
//!
//! Schema matches the legacy `runtara-agents/src/agents/crypto.rs` agent so
//! A/B parity tests can compare results byte-for-byte:
//! - `hash`: SHA-256 / SHA-512 / SHA-1 / MD5, hex or base64 output, string or
//!   base64-encoded FileData input.
//! - `hmac`: HMAC-SHA-256 / HMAC-SHA-512, hex or base64 output.

#![cfg(target_arch = "wasm32")]

#[allow(warnings)]
mod bindings;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use bindings::exports::runtara::agent::capabilities::{
    CapabilityInfo, ConnectionInfo, ErrorInfo, Guest, ModuleInfo,
};
use hmac::{Hmac, Mac};
use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};

// -----------------------------------------------------------------------------
// Input types (mirror runtara-agents/src/agents/crypto.rs)
// -----------------------------------------------------------------------------

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum HashDataInput {
    Text(String),
    File(FileData),
}

#[derive(serde::Deserialize)]
struct FileData {
    /// Base64-encoded content.
    content: String,
    #[serde(default)]
    #[allow(dead_code)]
    filename: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    mime_type: Option<String>,
}

impl HashDataInput {
    fn to_bytes(&self) -> Result<Vec<u8>, ErrorInfo> {
        match self {
            HashDataInput::Text(s) => Ok(s.as_bytes().to_vec()),
            HashDataInput::File(f) => BASE64.decode(&f.content).map_err(|e| ErrorInfo {
                code: "INVALID_BASE64".into(),
                message: format!("FileData.content is not valid base64: {e}"),
                category: "permanent".into(),
                severity: "error".into(),
                retryable: false,
                retry_after_ms: None,
                attributes: None,
            }),
        }
    }
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum HashAlgorithm {
    #[default]
    Sha256,
    Sha512,
    Sha1,
    Md5,
}

#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "lowercase")]
enum OutputFormat {
    #[default]
    Hex,
    Base64,
}

#[derive(serde::Deserialize, Default)]
enum HmacAlgorithm {
    #[default]
    #[serde(rename = "hmac-sha256")]
    HmacSha256,
    #[serde(rename = "hmac-sha512")]
    HmacSha512,
}

#[derive(serde::Deserialize)]
struct HashInput {
    data: HashDataInput,
    #[serde(default)]
    algorithm: HashAlgorithm,
    #[serde(default)]
    output_format: OutputFormat,
}

#[derive(serde::Deserialize)]
struct HmacInput {
    data: HashDataInput,
    secret: String,
    #[serde(default)]
    algorithm: HmacAlgorithm,
    #[serde(default)]
    output_format: OutputFormat,
}

#[derive(serde::Serialize)]
struct HashResult {
    hash: String,
    algorithm: String,
    format: String,
}

// -----------------------------------------------------------------------------
// Component plumbing
// -----------------------------------------------------------------------------

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
                description: Some(
                    "Hash data using SHA-256, SHA-512, SHA-1, or MD5. \
                     Accepts strings or base64-encoded files."
                        .into(),
                ),
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
                description: Some(
                    "Create HMAC authentication code using HMAC-SHA256 or HMAC-SHA512. \
                     Accepts strings or base64-encoded files."
                        .into(),
                ),
                has_side_effects: false,
                is_idempotent: true,
                rate_limited: false,
                tags: vec!["crypto".into()],
                input_schema: HMAC_INPUT_SCHEMA.into(),
                output_schema: HASH_OUTPUT_SCHEMA.into(),
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
            "hmac" => hmac_capability(&input),
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
    let input: HashInput = serde_json::from_str(input_json).map_err(bad_json)?;
    let data = input.data.to_bytes()?;
    let hash_bytes: Vec<u8> = match input.algorithm {
        HashAlgorithm::Sha256 => Sha256::digest(&data).to_vec(),
        HashAlgorithm::Sha512 => Sha512::digest(&data).to_vec(),
        HashAlgorithm::Sha1 => Sha1::digest(&data).to_vec(),
        HashAlgorithm::Md5 => Md5::digest(&data).to_vec(),
    };
    let out = HashResult {
        hash: format_hash(&hash_bytes, &input.output_format),
        algorithm: algorithm_name(&input.algorithm).into(),
        format: format_name(&input.output_format).into(),
    };
    serde_json::to_string(&out).map_err(bad_json)
}

fn hmac_capability(input_json: &str) -> Result<String, ErrorInfo> {
    let input: HmacInput = serde_json::from_str(input_json).map_err(bad_json)?;
    let data = input.data.to_bytes()?;
    let secret = input.secret.as_bytes();
    let hmac_bytes: Vec<u8> = match input.algorithm {
        HmacAlgorithm::HmacSha256 => {
            let mut mac = Hmac::<Sha256>::new_from_slice(secret).map_err(invalid_key)?;
            mac.update(&data);
            mac.finalize().into_bytes().to_vec()
        }
        HmacAlgorithm::HmacSha512 => {
            let mut mac = Hmac::<Sha512>::new_from_slice(secret).map_err(invalid_key)?;
            mac.update(&data);
            mac.finalize().into_bytes().to_vec()
        }
    };
    let out = HashResult {
        hash: format_hash(&hmac_bytes, &input.output_format),
        algorithm: hmac_algorithm_name(&input.algorithm).into(),
        format: format_name(&input.output_format).into(),
    };
    serde_json::to_string(&out).map_err(bad_json)
}

fn format_hash(bytes: &[u8], format: &OutputFormat) -> String {
    match format {
        OutputFormat::Hex => {
            let mut s = String::with_capacity(bytes.len() * 2);
            for b in bytes {
                s.push_str(&format!("{b:02x}"));
            }
            s
        }
        OutputFormat::Base64 => BASE64.encode(bytes),
    }
}

fn algorithm_name(algorithm: &HashAlgorithm) -> &'static str {
    match algorithm {
        HashAlgorithm::Sha256 => "sha256",
        HashAlgorithm::Sha512 => "sha512",
        HashAlgorithm::Sha1 => "sha1",
        HashAlgorithm::Md5 => "md5",
    }
}

fn hmac_algorithm_name(algorithm: &HmacAlgorithm) -> &'static str {
    match algorithm {
        HmacAlgorithm::HmacSha256 => "hmac-sha256",
        HmacAlgorithm::HmacSha512 => "hmac-sha512",
    }
}

fn format_name(format: &OutputFormat) -> &'static str {
    match format {
        OutputFormat::Hex => "hex",
        OutputFormat::Base64 => "base64",
    }
}

fn bad_json(e: serde_json::Error) -> ErrorInfo {
    ErrorInfo {
        code: "INPUT_DESERIALIZATION_ERROR".into(),
        message: e.to_string(),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
    }
}

fn invalid_key(e: hmac::digest::InvalidLength) -> ErrorInfo {
    ErrorInfo {
        code: "INVALID_KEY".into(),
        message: format!("Invalid HMAC key: {e}"),
        category: "permanent".into(),
        severity: "error".into(),
        retryable: false,
        retry_after_ms: None,
        attributes: None,
    }
}

// -----------------------------------------------------------------------------
// JSON Schemas published via list-capabilities()
// -----------------------------------------------------------------------------

const HASH_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["data"],
    "properties": {
        "data": {
            "oneOf": [
                { "type": "string", "description": "Data to hash (UTF-8 string)" },
                {
                    "type": "object",
                    "required": ["content"],
                    "properties": {
                        "content":   { "type": "string", "description": "Base64-encoded content" },
                        "filename":  { "type": "string" },
                        "mime_type": { "type": "string" }
                    }
                }
            ]
        },
        "algorithm": {
            "type": "string",
            "enum": ["sha256", "sha512", "sha1", "md5"],
            "default": "sha256"
        },
        "output_format": {
            "type": "string",
            "enum": ["hex", "base64"],
            "default": "hex"
        }
    }
}"#;

const HMAC_INPUT_SCHEMA: &str = r#"{
    "type": "object",
    "required": ["data", "secret"],
    "properties": {
        "data": {
            "oneOf": [
                { "type": "string" },
                {
                    "type": "object",
                    "required": ["content"],
                    "properties": {
                        "content":   { "type": "string", "description": "Base64-encoded content" },
                        "filename":  { "type": "string" },
                        "mime_type": { "type": "string" }
                    }
                }
            ]
        },
        "secret": { "type": "string", "description": "Secret key for HMAC" },
        "algorithm": {
            "type": "string",
            "enum": ["hmac-sha256", "hmac-sha512"],
            "default": "hmac-sha256"
        },
        "output_format": {
            "type": "string",
            "enum": ["hex", "base64"],
            "default": "hex"
        }
    }
}"#;

const HASH_OUTPUT_SCHEMA: &str = r#"{
    "type": "object",
    "properties": {
        "hash":      { "type": "string" },
        "algorithm": { "type": "string" },
        "format":    { "type": "string" }
    }
}"#;

bindings::export!(Component with_types_in bindings);
