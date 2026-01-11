// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! CryptoAgent - Cryptographic hashing operations
//!
//! This module provides hashing capabilities:
//! - hash: Hash data with SHA-256, SHA-512, SHA-1, or MD5
//! - hmac: Create HMAC authentication codes

use base64::{Engine as _, engine::general_purpose};
use hmac::{Hmac, Mac};
use md5::Md5;
use runtara_agent_macro::{CapabilityInput, CapabilityOutput, capability};
use runtara_dsl::agent_meta::EnumVariants;
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};
use strum::VariantNames;

use crate::types::FileData;

// ============================================================================
// Enums
// ============================================================================

/// Supported hash algorithms
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

/// Supported HMAC algorithms
#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, VariantNames)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
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

/// Output format for hash results
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

// ============================================================================
// Input Types
// ============================================================================

/// Flexible input that accepts both strings and FileData
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum HashDataInput {
    /// Plain text string
    Text(String),
    /// Base64-encoded file data
    File(FileData),
}

impl HashDataInput {
    /// Convert input to bytes for hashing
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        match self {
            HashDataInput::Text(s) => Ok(s.as_bytes().to_vec()),
            HashDataInput::File(f) => f.decode(),
        }
    }
}

/// Input for the hash capability
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "Hash Input")]
pub struct HashInput {
    /// The data to hash (string or file)
    #[field(
        display_name = "Data",
        description = "Data to hash - can be a string or a FileData object with base64-encoded content",
        example = "Hello World"
    )]
    pub data: HashDataInput,

    /// Hash algorithm to use
    #[field(
        display_name = "Algorithm",
        description = "Hash algorithm: sha256 (default), sha512, sha1, or md5",
        example = "sha256",
        default = "sha256",
        enum_type = "HashAlgorithm"
    )]
    #[serde(default)]
    pub algorithm: HashAlgorithm,

    /// Output format
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

/// Input for the HMAC capability
#[derive(Debug, Deserialize, CapabilityInput)]
#[capability_input(display_name = "HMAC Input")]
pub struct HmacInput {
    /// The data to authenticate
    #[field(
        display_name = "Data",
        description = "Data to create HMAC for - can be a string or a FileData object",
        example = "Hello World"
    )]
    pub data: HashDataInput,

    /// Secret key for HMAC
    #[field(
        display_name = "Secret Key",
        description = "Secret key for HMAC authentication",
        example = "my-secret-key"
    )]
    pub secret: String,

    /// HMAC algorithm to use
    #[field(
        display_name = "Algorithm",
        description = "HMAC algorithm: hmac-sha256 (default) or hmac-sha512",
        example = "hmac-sha256",
        default = "hmac-sha256",
        enum_type = "HmacAlgorithm"
    )]
    #[serde(default)]
    pub algorithm: HmacAlgorithm,

    /// Output format
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

// ============================================================================
// Output Types
// ============================================================================

/// Result of a hash operation
#[derive(Debug, Serialize, Deserialize, CapabilityOutput)]
#[capability_output(
    display_name = "Hash Result",
    description = "Result of hashing operation"
)]
pub struct HashResult {
    /// The computed hash value
    #[field(
        display_name = "Hash",
        description = "The computed hash value",
        example = "a591a6d40bf420404a011733cfb7b190d62c65bf0bcda32b57b277d9ad9f146e"
    )]
    pub hash: String,

    /// Algorithm used
    #[field(
        display_name = "Algorithm",
        description = "The hash algorithm used",
        example = "sha256"
    )]
    pub algorithm: String,

    /// Output format used
    #[field(
        display_name = "Format",
        description = "The output format (hex or base64)",
        example = "hex"
    )]
    pub format: String,
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Format hash bytes according to the output format
fn format_hash(bytes: &[u8], format: OutputFormat) -> String {
    match format {
        OutputFormat::Hex => bytes.iter().map(|b| format!("{:02x}", b)).collect(),
        OutputFormat::Base64 => general_purpose::STANDARD.encode(bytes),
    }
}

/// Get algorithm name as string
fn algorithm_name(algorithm: HashAlgorithm) -> &'static str {
    match algorithm {
        HashAlgorithm::Sha256 => "sha256",
        HashAlgorithm::Sha512 => "sha512",
        HashAlgorithm::Sha1 => "sha1",
        HashAlgorithm::Md5 => "md5",
    }
}

/// Get HMAC algorithm name as string
fn hmac_algorithm_name(algorithm: HmacAlgorithm) -> &'static str {
    match algorithm {
        HmacAlgorithm::HmacSha256 => "hmac-sha256",
        HmacAlgorithm::HmacSha512 => "hmac-sha512",
    }
}

/// Get output format name as string
fn format_name(format: OutputFormat) -> &'static str {
    match format {
        OutputFormat::Hex => "hex",
        OutputFormat::Base64 => "base64",
    }
}

// ============================================================================
// Capabilities
// ============================================================================

/// Hash data using various algorithms
#[capability(
    module = "crypto",
    display_name = "Hash",
    description = "Hash data using SHA-256, SHA-512, SHA-1, or MD5. Accepts strings or base64-encoded files."
)]
pub fn hash(input: HashInput) -> Result<HashResult, String> {
    let data = input.data.to_bytes()?;

    let hash_bytes: Vec<u8> = match input.algorithm {
        HashAlgorithm::Sha256 => {
            let mut hasher = Sha256::new();
            hasher.update(&data);
            hasher.finalize().to_vec()
        }
        HashAlgorithm::Sha512 => {
            let mut hasher = Sha512::new();
            hasher.update(&data);
            hasher.finalize().to_vec()
        }
        HashAlgorithm::Sha1 => {
            let mut hasher = Sha1::new();
            hasher.update(&data);
            hasher.finalize().to_vec()
        }
        HashAlgorithm::Md5 => {
            let mut hasher = Md5::new();
            hasher.update(&data);
            hasher.finalize().to_vec()
        }
    };

    Ok(HashResult {
        hash: format_hash(&hash_bytes, input.output_format),
        algorithm: algorithm_name(input.algorithm).to_string(),
        format: format_name(input.output_format).to_string(),
    })
}

/// Create HMAC authentication code
#[capability(
    module = "crypto",
    display_name = "HMAC",
    description = "Create HMAC authentication code using HMAC-SHA256 or HMAC-SHA512. Accepts strings or base64-encoded files."
)]
pub fn hmac(input: HmacInput) -> Result<HashResult, String> {
    let data = input.data.to_bytes()?;
    let secret = input.secret.as_bytes();

    let hmac_bytes: Vec<u8> = match input.algorithm {
        HmacAlgorithm::HmacSha256 => {
            let mut mac = Hmac::<Sha256>::new_from_slice(secret)
                .map_err(|e| format!("Invalid HMAC key: {}", e))?;
            mac.update(&data);
            mac.finalize().into_bytes().to_vec()
        }
        HmacAlgorithm::HmacSha512 => {
            let mut mac = Hmac::<Sha512>::new_from_slice(secret)
                .map_err(|e| format!("Invalid HMAC key: {}", e))?;
            mac.update(&data);
            mac.finalize().into_bytes().to_vec()
        }
    };

    Ok(HashResult {
        hash: format_hash(&hmac_bytes, input.output_format),
        algorithm: hmac_algorithm_name(input.algorithm).to_string(),
        format: format_name(input.output_format).to_string(),
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_sha256_string() {
        let input = HashInput {
            data: HashDataInput::Text("Hello World".to_string()),
            algorithm: HashAlgorithm::Sha256,
            output_format: OutputFormat::Hex,
        };
        let result = hash(input).unwrap();
        assert_eq!(result.algorithm, "sha256");
        assert_eq!(result.format, "hex");
        // SHA-256 of "Hello World"
        assert_eq!(
            result.hash,
            "a591a6d40bf420404a011733cfb7b190d62c65bf0bcda32b57b277d9ad9f146e"
        );
    }

    #[test]
    fn test_hash_sha512_string() {
        let input = HashInput {
            data: HashDataInput::Text("Hello World".to_string()),
            algorithm: HashAlgorithm::Sha512,
            output_format: OutputFormat::Hex,
        };
        let result = hash(input).unwrap();
        assert_eq!(result.algorithm, "sha512");
        assert!(result.hash.len() == 128); // SHA-512 produces 64 bytes = 128 hex chars
    }

    #[test]
    fn test_hash_sha1_string() {
        let input = HashInput {
            data: HashDataInput::Text("Hello World".to_string()),
            algorithm: HashAlgorithm::Sha1,
            output_format: OutputFormat::Hex,
        };
        let result = hash(input).unwrap();
        assert_eq!(result.algorithm, "sha1");
        // SHA-1 of "Hello World"
        assert_eq!(result.hash, "0a4d55a8d778e5022fab701977c5d840bbc486d0");
    }

    #[test]
    fn test_hash_md5_string() {
        let input = HashInput {
            data: HashDataInput::Text("Hello World".to_string()),
            algorithm: HashAlgorithm::Md5,
            output_format: OutputFormat::Hex,
        };
        let result = hash(input).unwrap();
        assert_eq!(result.algorithm, "md5");
        // MD5 of "Hello World"
        assert_eq!(result.hash, "b10a8db164e0754105b7a99be72e3fe5");
    }

    #[test]
    fn test_hash_base64_output() {
        let input = HashInput {
            data: HashDataInput::Text("Hello World".to_string()),
            algorithm: HashAlgorithm::Sha256,
            output_format: OutputFormat::Base64,
        };
        let result = hash(input).unwrap();
        assert_eq!(result.format, "base64");
        // Base64 encoding of SHA-256 hash
        assert_eq!(result.hash, "pZGm1Av0IEBKARczz7exkNYsZb8LzaMrV7J32a2fFG4=");
    }

    #[test]
    fn test_hash_file_data() {
        // "Hello World" encoded as base64
        let file_data = FileData {
            content: "SGVsbG8gV29ybGQ=".to_string(),
            filename: Some("test.txt".to_string()),
            mime_type: Some("text/plain".to_string()),
        };
        let input = HashInput {
            data: HashDataInput::File(file_data),
            algorithm: HashAlgorithm::Sha256,
            output_format: OutputFormat::Hex,
        };
        let result = hash(input).unwrap();
        // Should produce same hash as the string "Hello World"
        assert_eq!(
            result.hash,
            "a591a6d40bf420404a011733cfb7b190d62c65bf0bcda32b57b277d9ad9f146e"
        );
    }

    #[test]
    fn test_hmac_sha256() {
        let input = HmacInput {
            data: HashDataInput::Text("Hello World".to_string()),
            secret: "secret-key".to_string(),
            algorithm: HmacAlgorithm::HmacSha256,
            output_format: OutputFormat::Hex,
        };
        let result = hmac(input).unwrap();
        assert_eq!(result.algorithm, "hmac-sha256");
        assert_eq!(result.format, "hex");
        assert_eq!(result.hash.len(), 64); // HMAC-SHA256 produces 32 bytes = 64 hex chars
    }

    #[test]
    fn test_hmac_sha512() {
        let input = HmacInput {
            data: HashDataInput::Text("Hello World".to_string()),
            secret: "secret-key".to_string(),
            algorithm: HmacAlgorithm::HmacSha512,
            output_format: OutputFormat::Hex,
        };
        let result = hmac(input).unwrap();
        assert_eq!(result.algorithm, "hmac-sha512");
        assert_eq!(result.hash.len(), 128); // HMAC-SHA512 produces 64 bytes = 128 hex chars
    }

    #[test]
    fn test_hmac_base64_output() {
        let input = HmacInput {
            data: HashDataInput::Text("Hello World".to_string()),
            secret: "secret-key".to_string(),
            algorithm: HmacAlgorithm::HmacSha256,
            output_format: OutputFormat::Base64,
        };
        let result = hmac(input).unwrap();
        assert_eq!(result.format, "base64");
    }

    #[test]
    fn test_hash_empty_string() {
        let input = HashInput {
            data: HashDataInput::Text("".to_string()),
            algorithm: HashAlgorithm::Sha256,
            output_format: OutputFormat::Hex,
        };
        let result = hash(input).unwrap();
        // SHA-256 of empty string
        assert_eq!(
            result.hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}
