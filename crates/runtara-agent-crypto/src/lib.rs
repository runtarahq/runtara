//! Crypto agent — hashing and HMAC — as a WebAssembly component.
//!
//! Capability metadata travels through `#[capability_input]` / `#[capability]` /
//! `#[capability_output]` annotations on the same Rust types and functions that
//! the wasm cdylib's `invoke` dispatcher calls into. The workspace binary
//! `runtara-agent-bundle-emit` reads these macro-emitted `&'static` statics on
//! the host architecture and writes `runtara_agent_crypto.meta.json` next to
//! the `.wasm` — the JSON is a build artifact, never hand-edited.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use hmac::{Hmac, Mac};
use md5::Md5;
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use runtara_dsl::agent_meta::EnumVariants;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};
use strum::VariantNames;

#[cfg(target_arch = "wasm32")]
#[allow(warnings)]
mod bindings;

// -----------------------------------------------------------------------------
// Enums (with VariantNames + EnumVariants so the macro can record allowed values)
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, VariantNames)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum HashAlgorithm {
    #[default]
    Sha256,
    Sha512,
    Sha1,
    Md5,
}
impl EnumVariants for HashAlgorithm {
    fn variant_names() -> &'static [&'static str] {
        Self::VARIANTS
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, VariantNames)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum OutputFormat {
    #[default]
    Hex,
    Base64,
}
impl EnumVariants for OutputFormat {
    fn variant_names() -> &'static [&'static str] {
        Self::VARIANTS
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, VariantNames)]
pub enum HmacAlgorithm {
    #[default]
    #[serde(rename = "hmac-sha256")]
    #[strum(serialize = "hmac-sha256")]
    HmacSha256,
    #[serde(rename = "hmac-sha512")]
    #[strum(serialize = "hmac-sha512")]
    HmacSha512,
}
impl EnumVariants for HmacAlgorithm {
    fn variant_names() -> &'static [&'static str] {
        Self::VARIANTS
    }
}

// -----------------------------------------------------------------------------
// Inputs / outputs (with capability macros so meta.json can be derived)
// -----------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum HashDataInput {
    Text(String),
    File(FileData),
}

#[derive(Debug, Deserialize)]
pub struct FileData {
    pub content: String,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub mime_type: Option<String>,
}

impl HashDataInput {
    fn to_bytes(&self) -> Result<Vec<u8>, String> {
        match self {
            HashDataInput::Text(s) => Ok(s.as_bytes().to_vec()),
            HashDataInput::File(f) => BASE64.decode(&f.content).map_err(|e| {
                serde_json::json!({
                    "code": "INVALID_BASE64",
                    "message": format!("FileData.content is not valid base64: {e}"),
                    "category": "permanent",
                    "severity": "error",
                })
                .to_string()
            }),
        }
    }
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Hash Input")]
pub struct HashInput {
    #[field(
        display_name = "Data",
        description = "Data to hash — can be a string or a FileData object with base64-encoded content",
        example = "Hello World"
    )]
    pub data: HashDataInput,

    #[field(
        display_name = "Algorithm",
        description = "Hash algorithm: sha256 (default), sha512, sha1, or md5",
        example = "sha256",
        default = "sha256",
        enum_type = "HashAlgorithm"
    )]
    #[serde(default)]
    pub algorithm: HashAlgorithm,

    #[field(
        display_name = "Output Format",
        description = "Output format: hex (default) or base64",
        example = "hex",
        default = "hex",
        enum_type = "OutputFormat"
    )]
    #[serde(default)]
    pub output_format: OutputFormat,
}

#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "HMAC Input")]
pub struct HmacInput {
    #[field(
        display_name = "Data",
        description = "Data to create HMAC for — can be a string or a FileData object",
        example = "Hello World"
    )]
    pub data: HashDataInput,

    #[field(
        display_name = "Secret Key",
        description = "Secret key for HMAC authentication",
        example = "my-secret-key"
    )]
    pub secret: String,

    #[field(
        display_name = "Algorithm",
        description = "HMAC algorithm: hmac-sha256 (default) or hmac-sha512",
        example = "hmac-sha256",
        default = "hmac-sha256",
        enum_type = "HmacAlgorithm"
    )]
    #[serde(default)]
    pub algorithm: HmacAlgorithm,

    #[field(
        display_name = "Output Format",
        description = "Output format: hex (default) or base64",
        example = "hex",
        default = "hex",
        enum_type = "OutputFormat"
    )]
    #[serde(default)]
    pub output_format: OutputFormat,
}

#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Hash Result",
    description = "Result of hashing or HMAC operation"
)]
pub struct HashResult {
    #[field(
        display_name = "Hash",
        description = "The computed hash or HMAC value",
        example = "a591a6d40bf420404a011733cfb7b190d62c65bf0bcda32b57b277d9ad9f146e"
    )]
    pub hash: String,

    #[field(
        display_name = "Algorithm",
        description = "The algorithm used",
        example = "sha256"
    )]
    pub algorithm: String,

    #[field(
        display_name = "Format",
        description = "The output format (hex or base64)",
        example = "hex"
    )]
    pub format: String,
}

// -----------------------------------------------------------------------------
// Capabilities — annotated for metadata; the `__executor_*` fns the macro emits
// are what the wasm Guest impl dispatches to.
// -----------------------------------------------------------------------------

#[capability(
    id = "hash",
    module = "crypto",
    module_display_name = "Crypto",
    module_description = "Hashing and HMAC primitives.",
    display_name = "Hash",
    description = "Hash data using SHA-256, SHA-512, SHA-1, or MD5. Accepts strings or base64-encoded files."
)]
pub fn hash(input: HashInput) -> Result<HashResult, String> {
    let data = input.data.to_bytes()?;
    let hash_bytes: Vec<u8> = match input.algorithm {
        HashAlgorithm::Sha256 => Sha256::digest(&data).to_vec(),
        HashAlgorithm::Sha512 => Sha512::digest(&data).to_vec(),
        HashAlgorithm::Sha1 => Sha1::digest(&data).to_vec(),
        HashAlgorithm::Md5 => Md5::digest(&data).to_vec(),
    };
    Ok(HashResult {
        hash: format_hash(&hash_bytes, input.output_format),
        algorithm: algorithm_name(input.algorithm).into(),
        format: format_name(input.output_format).into(),
    })
}

#[capability(
    id = "hmac",
    module = "crypto",
    display_name = "HMAC",
    description = "Create HMAC authentication code using HMAC-SHA256 or HMAC-SHA512. Accepts strings or base64-encoded files."
)]
pub fn hmac_capability(input: HmacInput) -> Result<HashResult, String> {
    let data = input.data.to_bytes()?;
    let secret = input.secret.as_bytes();
    let hmac_bytes: Vec<u8> = match input.algorithm {
        HmacAlgorithm::HmacSha256 => {
            let mut mac = Hmac::<Sha256>::new_from_slice(secret).map_err(|e| {
                serde_json::json!({
                    "code": "INVALID_KEY",
                    "message": format!("Invalid HMAC key: {e}"),
                    "category": "permanent",
                    "severity": "error",
                })
                .to_string()
            })?;
            mac.update(&data);
            mac.finalize().into_bytes().to_vec()
        }
        HmacAlgorithm::HmacSha512 => {
            let mut mac = Hmac::<Sha512>::new_from_slice(secret).map_err(|e| {
                serde_json::json!({
                    "code": "INVALID_KEY",
                    "message": format!("Invalid HMAC key: {e}"),
                    "category": "permanent",
                    "severity": "error",
                })
                .to_string()
            })?;
            mac.update(&data);
            mac.finalize().into_bytes().to_vec()
        }
    };
    Ok(HashResult {
        hash: format_hash(&hmac_bytes, input.output_format),
        algorithm: hmac_algorithm_name(input.algorithm).into(),
        format: format_name(input.output_format).into(),
    })
}

fn format_hash(bytes: &[u8], format: OutputFormat) -> String {
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

fn algorithm_name(algorithm: HashAlgorithm) -> &'static str {
    match algorithm {
        HashAlgorithm::Sha256 => "sha256",
        HashAlgorithm::Sha512 => "sha512",
        HashAlgorithm::Sha1 => "sha1",
        HashAlgorithm::Md5 => "md5",
    }
}

fn hmac_algorithm_name(algorithm: HmacAlgorithm) -> &'static str {
    match algorithm {
        HmacAlgorithm::HmacSha256 => "hmac-sha256",
        HmacAlgorithm::HmacSha512 => "hmac-sha512",
    }
}

fn format_name(format: OutputFormat) -> &'static str {
    match format {
        OutputFormat::Hex => "hex",
        OutputFormat::Base64 => "base64",
    }
}

// -----------------------------------------------------------------------------
// AgentInfo assembler (host-only; the wasm binary doesn't need it)
// -----------------------------------------------------------------------------

/// Build the canonical `AgentInfo` for this agent by walking the macro-emitted
/// `&'static` statics. The workspace `runtara-agent-bundle-emit` binary calls
/// this on the host architecture and writes the JSON to disk; the wasm binary
/// itself never executes this code, so we cfg-gate it out to keep the
/// component small.
#[cfg(not(target_arch = "wasm32"))]
pub fn agent_info() -> runtara_dsl::agent_meta::AgentInfo {
    use runtara_dsl::agent_meta::{
        AgentInfo, CapabilityMeta, InputTypeMeta, OutputTypeMeta, capability_to_api,
    };
    use std::collections::HashMap;

    let caps: &[&'static CapabilityMeta] =
        &[&__CAPABILITY_META_HASH, &__CAPABILITY_META_HMAC_CAPABILITY];
    let input_types: HashMap<&'static str, &'static InputTypeMeta> = [
        ("HashInput", &__INPUT_META_HashInput as &InputTypeMeta),
        ("HmacInput", &__INPUT_META_HmacInput),
    ]
    .into_iter()
    .collect();
    let output_types: HashMap<&'static str, &'static OutputTypeMeta> =
        [("HashResult", &__OUTPUT_META_HashResult as &OutputTypeMeta)]
            .into_iter()
            .collect();

    let capabilities = caps
        .iter()
        .map(|cap| {
            capability_to_api(
                cap,
                input_types.get(cap.input_type).copied(),
                output_types.get(cap.output_type).copied(),
            )
        })
        .collect();

    AgentInfo {
        id: "crypto".into(),
        name: "Crypto".into(),
        description: "Hashing and HMAC primitives.".into(),
        has_side_effects: false,
        supports_connections: false,
        integration_ids: vec![],
        capabilities,
    }
}

// -----------------------------------------------------------------------------
// Wasm component plumbing
// -----------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
// Per-agent WIT layout: this agent's `capabilities` interface lives under
// `runtara:agent-crypto`, so cargo-component generates the export bindings
// under `bindings::exports::runtara::agent_crypto::capabilities`. Shared
// records (ConnectionInfo / ErrorInfo) are re-exported there too via the WIT
// `use runtara:agent/types@0.3.0.{…};` import.
use bindings::exports::runtara::agent_crypto::capabilities::{ConnectionInfo, ErrorInfo, Guest};

#[cfg(target_arch = "wasm32")]
struct Component;

#[cfg(target_arch = "wasm32")]
impl Guest for Component {
    fn invoke(
        capability_id: String,
        input: Vec<u8>,
        _connection: Option<ConnectionInfo>,
    ) -> Result<Vec<u8>, ErrorInfo> {
        let value: serde_json::Value = serde_json::from_slice(&input).map_err(bad_json)?;
        let executor_result = match capability_id.as_str() {
            "hash" => __executor_hash(value),
            "hmac" => __executor_hmac_capability(value),
            other => {
                return Err(ErrorInfo {
                    code: "UNKNOWN_CAPABILITY".into(),
                    message: format!("crypto agent has no capability `{other}`"),
                    category: "permanent".into(),
                    severity: "error".into(),
                    retryable: false,
                    retry_after_ms: None,
                    attributes: None,
                });
            }
        };
        executor_result
            .map_err(error_string_to_error_info)
            .and_then(|out_value| serde_json::to_vec(&out_value).map_err(bad_json))
    }
}

#[cfg(target_arch = "wasm32")]
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

/// The `#[capability]` macro packages each error as a JSON-string with
/// `{ code, message, category, severity }`. Parse it back into a typed
/// `ErrorInfo` for the WIT result.
#[cfg(target_arch = "wasm32")]
fn error_string_to_error_info(s: String) -> ErrorInfo {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&s) {
        ErrorInfo {
            code: value
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("CAPABILITY_ERROR")
                .into(),
            message: value
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or(&s)
                .into(),
            category: value
                .get("category")
                .and_then(|v| v.as_str())
                .unwrap_or("permanent")
                .into(),
            severity: value
                .get("severity")
                .and_then(|v| v.as_str())
                .unwrap_or("error")
                .into(),
            retryable: value
                .get("retryable")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            retry_after_ms: value.get("retry_after_ms").and_then(|v| v.as_u64()),
            attributes: value.get("attributes").map(|v| v.to_string()),
        }
    } else {
        ErrorInfo {
            code: "CAPABILITY_ERROR".into(),
            message: s,
            category: "permanent".into(),
            severity: "error".into(),
            retryable: false,
            retry_after_ms: None,
            attributes: None,
        }
    }
}

#[cfg(target_arch = "wasm32")]
bindings::export!(Component with_types_in bindings);
