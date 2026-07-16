// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Agent capability metadata types for runtime introspection
//!
//! These types are used by the `runtara-agent-macro` crate to generate
//! named static metadata. Runtime crates build explicit static registries from
//! those symbols for deterministic native and WASM discovery.

/// Trait for types that can provide their enum variant names.
/// Used by the CapabilityInput macro to extract enum values for API metadata.
pub trait EnumVariants {
    /// Returns the variant names as they appear in JSON serialization
    fn variant_names() -> &'static [&'static str];
}

/// Function pointer type for getting enum variant names
pub type EnumVariantsFn = fn() -> &'static [&'static str];

/// Synchronous executor function type for agent capabilities.
pub type CapabilityExecutorFn = fn(serde_json::Value) -> Result<serde_json::Value, String>;

/// Executor for an agent capability.
pub struct CapabilityExecutor {
    /// The agent module name (e.g., "utils", "transform")
    pub module: &'static str,
    /// Capability ID in kebab-case (e.g., "random-double")
    pub capability_id: &'static str,
    /// The executor function
    pub execute: CapabilityExecutorFn,
}

/// Execute a capability by module and capability_id.
///
/// Agent execution is provided by `runtara-agents::registry`. This fallback
/// remains for older callers that still compile against `runtara-dsl` directly.
pub fn execute_capability(
    module: &str,
    capability_id: &str,
    _input: serde_json::Value,
) -> Result<serde_json::Value, String> {
    Err(format!("Unknown capability: {}:{}", module, capability_id))
}

/// Hint for how to compensate (undo) a capability's effects.
///
/// This is **metadata only** - the system never auto-compensates.
/// Tools can use this to suggest compensation configurations to users.
///
/// Note: The actual compensation data mapping is defined by the workflow author
/// in the workflow definition, not here. This hint only suggests which capability
/// can reverse the effects - the author decides how to wire up the inputs.
#[derive(Debug, Clone)]
pub struct CompensationHint {
    /// The capability ID that reverses this capability's effects.
    /// Must be in the same module (e.g., "release" for "reserve").
    pub capability_id: &'static str,
    /// Human-readable description of what the compensation does.
    pub description: Option<&'static str>,
}

/// Metadata for an agent capability
#[derive(Debug, Clone)]
pub struct CapabilityMeta {
    /// The agent module name (e.g., "utils", "transform")
    pub module: Option<&'static str>,
    /// Capability ID in kebab-case (e.g., "random-double")
    pub capability_id: &'static str,
    /// The Rust function name (e.g., "random_double")
    pub function_name: &'static str,
    /// Input type name (e.g., "RandomDoubleInput")
    pub input_type: &'static str,
    /// Output type name (e.g., "f64", "Value")
    pub output_type: &'static str,
    /// Display name for UI
    pub display_name: Option<&'static str>,
    /// Description of the capability
    pub description: Option<&'static str>,
    /// Whether this capability has side effects
    pub has_side_effects: bool,
    /// Whether this capability is idempotent
    pub is_idempotent: bool,
    /// Whether this capability requires rate limiting (external API calls)
    pub rate_limited: bool,
    /// Optional compensation hint - suggests how to undo this capability's effects.
    /// This is metadata only; the system never auto-compensates.
    pub compensation_hint: Option<CompensationHint>,
    /// Known errors this capability can return.
    /// Used for tooling hints, validation, and documentation generation.
    pub known_errors: &'static [KnownError],
    /// Semantic tags for capability classification and filtering.
    /// Well-known tags: "memory:read", "memory:write".
    pub tags: &'static [&'static str],
}

/// Well-known capability tags
pub mod capability_tags {
    /// Capability can load/read conversation memory
    pub const MEMORY_READ: &str = "memory:read";
    /// Capability can save/write conversation memory
    pub const MEMORY_WRITE: &str = "memory:write";
}

/// Error category for capability errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Transient error - retry is likely to succeed
    Transient,
    /// Permanent error - don't retry
    Permanent,
}

impl ErrorKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorKind::Transient => "transient",
            ErrorKind::Permanent => "permanent",
        }
    }
}

/// A known error that a capability can return.
/// Used for compile-time introspection and tooling.
#[derive(Debug, Clone)]
pub struct KnownError {
    /// Machine-readable error code (e.g., "HTTP_TIMEOUT", "SFTP_AUTH_ERROR")
    pub code: &'static str,
    /// Human-readable description of when this error occurs
    pub description: &'static str,
    /// Error category (transient or permanent)
    pub kind: ErrorKind,
    /// Context attributes that are included with this error (e.g., ["host", "port"])
    pub attributes: &'static [&'static str],
}

/// Metadata for an input field
#[derive(Clone)]
pub struct InputFieldMeta {
    /// Field name
    pub name: &'static str,
    /// Type name (without Option wrapper)
    pub type_name: &'static str,
    /// Whether this field is optional
    pub is_optional: bool,
    /// Display name for UI
    pub display_name: Option<&'static str>,
    /// Description of the field
    pub description: Option<&'static str>,
    /// Example value
    pub example: Option<&'static str>,
    /// Default value as JSON string
    pub default_value: Option<&'static str>,
    /// Function to get enum values (for types implementing EnumVariants)
    pub enum_values_fn: Option<EnumVariantsFn>,
}

impl std::fmt::Debug for InputFieldMeta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputFieldMeta")
            .field("name", &self.name)
            .field("type_name", &self.type_name)
            .field("is_optional", &self.is_optional)
            .field("display_name", &self.display_name)
            .field("description", &self.description)
            .field("example", &self.example)
            .field("default_value", &self.default_value)
            .field("enum_values_fn", &self.enum_values_fn.map(|_| "<fn>"))
            .finish()
    }
}

/// Metadata for an input type (struct)
#[derive(Debug, Clone)]
pub struct InputTypeMeta {
    /// Type name (e.g., "RandomDoubleInput")
    pub type_name: &'static str,
    /// Display name for UI
    pub display_name: Option<&'static str>,
    /// Description of the type
    pub description: Option<&'static str>,
    /// Fields in this type
    pub fields: &'static [InputFieldMeta],
}

/// Metadata for an output field
#[derive(Debug, Clone)]
pub struct OutputFieldMeta {
    /// Field name
    pub name: &'static str,
    /// Type name (e.g., "String", "Vec<Product>", "Option<i32>")
    pub type_name: &'static str,
    /// Display name for UI
    pub display_name: Option<&'static str>,
    /// Description of the field
    pub description: Option<&'static str>,
    /// Example value
    pub example: Option<&'static str>,
    /// Whether this field can be null (true for Option<T> types)
    pub nullable: bool,
    /// For array types (Vec<T>), describes the item type name
    /// This is the type name that can be looked up in the output types registry
    pub items_type_name: Option<&'static str>,
    /// For nested object types, the type name that can be looked up in the output types registry
    /// This enables recursive type resolution for complex nested structures
    pub nested_type_name: Option<&'static str>,
}

/// Metadata for an output type (struct)
#[derive(Debug, Clone)]
pub struct OutputTypeMeta {
    /// Type name (e.g., "RandomDoubleOutput")
    pub type_name: &'static str,
    /// Display name for UI
    pub display_name: Option<&'static str>,
    /// Description of the type
    pub description: Option<&'static str>,
    /// Fields in this type
    pub fields: &'static [OutputFieldMeta],
}

/// Get all registered capability metadata
pub fn get_all_capabilities() -> std::iter::Empty<&'static CapabilityMeta> {
    std::iter::empty()
}

/// Get all registered input type metadata
pub fn get_all_input_types() -> std::iter::Empty<&'static InputTypeMeta> {
    std::iter::empty()
}

/// Get all registered output type metadata
pub fn get_all_output_types() -> std::iter::Empty<&'static OutputTypeMeta> {
    std::iter::empty()
}

/// Find input type metadata by type name
pub fn find_input_type(type_name: &str) -> Option<&'static InputTypeMeta> {
    get_all_input_types().find(|m| m.type_name == type_name)
}

/// Find output type metadata by type name
pub fn find_output_type(type_name: &str) -> Option<&'static OutputTypeMeta> {
    get_all_output_types().find(|m| m.type_name == type_name)
}

// ============================================================================
// API-Compatible Types (for REST API serialization)
// ============================================================================

use serde::{Deserialize, Serialize};

/// API-compatible agent info
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(rename = "hasSideEffects")]
    pub has_side_effects: bool,
    #[serde(rename = "supportsConnections")]
    pub supports_connections: bool,
    #[serde(rename = "integrationIds")]
    pub integration_ids: Vec<String>,
    pub capabilities: Vec<CapabilityInfo>,
}

/// API-compatible compensation hint info
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct CompensationHintInfo {
    /// Capability ID that reverses this capability's effects
    #[serde(rename = "capabilityId")]
    pub capability_id: String,
    /// Human-readable description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// API-compatible known error info
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct KnownErrorInfo {
    /// Machine-readable error code (e.g., "HTTP_TIMEOUT")
    pub code: String,
    /// Human-readable description of when this error occurs
    pub description: String,
    /// Error kind: "transient" or "permanent"
    pub kind: String,
    /// Context attributes included with this error
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attributes: Vec<String>,
}

impl From<&KnownError> for KnownErrorInfo {
    fn from(err: &KnownError) -> Self {
        KnownErrorInfo {
            code: err.code.to_string(),
            description: err.description.to_string(),
            kind: err.kind.as_str().to_string(),
            attributes: err.attributes.iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// API-compatible capability info
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct CapabilityInfo {
    pub id: String,
    pub name: String,
    #[serde(rename = "displayName", skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "inputType")]
    pub input_type: String,
    pub inputs: Vec<CapabilityField>,
    pub output: FieldTypeInfo,
    #[serde(rename = "hasSideEffects")]
    pub has_side_effects: bool,
    #[serde(rename = "isIdempotent")]
    pub is_idempotent: bool,
    #[serde(rename = "rateLimited")]
    pub rate_limited: bool,
    /// Optional compensation hint - suggests how to undo this capability.
    /// This is metadata only; the system never auto-compensates.
    #[serde(rename = "compensationHint", skip_serializing_if = "Option::is_none")]
    pub compensation_hint: Option<CompensationHintInfo>,
    /// Known errors this capability can return.
    /// Used for tooling hints and documentation.
    #[serde(default, rename = "knownErrors", skip_serializing_if = "Vec::is_empty")]
    pub known_errors: Vec<KnownErrorInfo>,
    /// Semantic tags for capability classification and filtering.
    /// Well-known tags: "memory:read", "memory:write".
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// API-compatible capability field info.
/// Used for agent inputs and workflow input/output schemas.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct CapabilityField {
    pub name: String,
    #[serde(rename = "displayName", skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub type_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<FieldTypeInfo>,
    pub required: bool,
    #[serde(rename = "default", skip_serializing_if = "Option::is_none")]
    pub default_value: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<String>>,
    /// JSON Schema for complex object types (e.g., ConditionExpression)
    /// Provides detailed structure hints for strongly-typed objects.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
}

/// API-compatible field type info.
/// Describes the type of a field, including nested structures.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct FieldTypeInfo {
    #[serde(rename = "type")]
    pub type_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(rename = "displayName", skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Sidecar meta.json carries this; legacy registry populates it server-side
    /// from `OutputTypeMeta`. Either way, the round-trip via serde is symmetric.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "utoipa", schema(no_recursion))]
    pub fields: Option<Box<Vec<OutputField>>>,
    /// For array types, describes the item type
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "utoipa", schema(no_recursion))]
    pub items: Option<Box<FieldTypeInfo>>,
    /// Whether this field can be null
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub nullable: bool,
}

/// API-compatible output field info.
/// Describes an output field with type information.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct OutputField {
    pub name: String,
    #[serde(rename = "displayName", skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub type_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub example: Option<serde_json::Value>,
    /// Whether this field can be null
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub nullable: bool,
    /// For array types, describes the item type
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "utoipa", schema(no_recursion))]
    pub items: Option<Box<FieldTypeInfo>>,
    /// For nested object types, the fields of the nested object
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "utoipa", schema(no_recursion))]
    pub fields: Option<Box<Vec<OutputField>>>,
}

// ============================================================================
// Agent Module Configuration (static metadata not derivable from macros)
// ============================================================================

/// Static configuration for agent modules
#[derive(Debug, Clone)]
pub struct AgentModuleConfig {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub has_side_effects: bool,
    pub supports_connections: bool,
    pub integration_ids: &'static [&'static str],
    /// Whether this agent can receive sensitive connection data from Connection steps.
    /// Only secure agents (http, sftp) should have this set to true.
    /// This prevents connection credentials from leaking through non-secure agents.
    pub secure: bool,
}

/// Built-in agent module configurations
pub const BUILTIN_AGENT_MODULES: &[AgentModuleConfig] = &[
    AgentModuleConfig {
        id: "utils",
        name: "Utils",
        description: "Utility capabilities for random numbers, calculations, delays, timestamps, and country lookups",
        has_side_effects: false,
        supports_connections: false,
        integration_ids: &[],
        secure: false,
    },
    AgentModuleConfig {
        id: "transform",
        name: "Transform",
        description: "Transform capabilities for data manipulation, filtering, sorting, and JSON operations",
        has_side_effects: false,
        supports_connections: false,
        integration_ids: &[],
        secure: false,
    },
    AgentModuleConfig {
        id: "crypto",
        name: "Crypto",
        description: "Cryptographic capabilities for hashing data with SHA-256, SHA-512, SHA-1, MD5 and creating HMAC authentication codes",
        has_side_effects: false,
        supports_connections: false,
        integration_ids: &[],
        secure: false,
    },
    AgentModuleConfig {
        id: "csv",
        name: "Csv",
        description: "CSV capabilities for parsing and working with CSV data",
        has_side_effects: false,
        supports_connections: false,
        integration_ids: &[],
        secure: false,
    },
    AgentModuleConfig {
        id: "text",
        name: "Text",
        description: "Text capabilities for string manipulation, formatting, and text processing",
        has_side_effects: false,
        supports_connections: false,
        integration_ids: &[],
        secure: false,
    },
    AgentModuleConfig {
        id: "xml",
        name: "Xml",
        description: "XML capabilities for parsing and working with XML data",
        has_side_effects: false,
        supports_connections: false,
        integration_ids: &[],
        secure: false,
    },
    AgentModuleConfig {
        id: "datetime",
        name: "DateTime",
        description: "Date and time capabilities for parsing, formatting, calculating, and manipulating dates",
        has_side_effects: false,
        supports_connections: false,
        integration_ids: &[],
        secure: false,
    },
    AgentModuleConfig {
        id: "http",
        name: "HTTP",
        description: "HTTP capabilities for making web requests with JSON/text/binary support (has side effects)",
        has_side_effects: true,
        supports_connections: true,
        integration_ids: &[
            "http_bearer",
            "http_api_key",
            "microsoft_entra_client_credentials",
        ],
        secure: true,
    },
    AgentModuleConfig {
        id: "sftp",
        name: "Sftp",
        description: "SFTP capabilities for secure file transfer operations - list, download, upload, and delete files on remote servers (has side effects)",
        has_side_effects: true,
        supports_connections: true,
        integration_ids: &["sftp"],
        secure: true,
    },
    AgentModuleConfig {
        id: "compression",
        name: "Compression",
        description: "Archive capabilities for creating and extracting ZIP archives, listing contents, and extracting individual files",
        has_side_effects: false,
        supports_connections: false,
        integration_ids: &[],
        secure: false,
    },
    AgentModuleConfig {
        id: "object-model",
        name: "Object Model",
        description: "Object Model capabilities for database CRUD operations - create, query, and check instances in object model schemas (has side effects)",
        has_side_effects: true,
        supports_connections: true,
        integration_ids: &["postgres"],
        secure: false,
    },
];

/// Get built-in agent modules.
///
/// Full agent registries are provided by `runtara-agents::registry`.
pub fn get_all_agent_modules() -> Vec<&'static AgentModuleConfig> {
    BUILTIN_AGENT_MODULES.iter().collect()
}

/// Find agent module config by id (matched canonically; see
/// [`canonical_agent_id`], so legacy `object_model` resolves to
/// `object-model`).
pub fn find_agent_module(id: &str) -> Option<&'static AgentModuleConfig> {
    let query = canonical_agent_id(id);
    get_all_agent_modules()
        .into_iter()
        .find(|m| canonical_agent_id(m.id) == query)
}

// ============================================================================
// Step Type Metadata (for automatic DSL generation)
// ============================================================================
//
// The whole block is gated behind `json-schema` because `SchemaGeneratorFn`
// returns `schemars::Schema`. WASM consumers (e.g.
// `runtara-report-dsl`) build with `default-features = false` to keep
// `schemars` out of their tree.

/// Function pointer type for generating JSON schema
#[cfg(feature = "json-schema")]
pub type SchemaGeneratorFn = fn() -> schemars::Schema;

/// Metadata for a step type.
#[cfg(feature = "json-schema")]
#[derive(Debug, Clone)]
pub struct StepTypeMeta {
    /// Step type ID in PascalCase (e.g., "Conditional", "Agent")
    pub id: &'static str,
    /// Display name for UI
    pub display_name: &'static str,
    /// Description of the step type
    pub description: &'static str,
    /// Category: "control" or "execution"
    pub category: &'static str,
    /// Function to generate JSON Schema for this step type
    pub schema_fn: SchemaGeneratorFn,
}

/// Get all registered step type metadata
#[cfg(feature = "json-schema")]
pub fn get_all_step_types() -> impl Iterator<Item = &'static StepTypeMeta> {
    crate::step_registration::STEP_TYPES.iter().copied()
}

/// Find step type metadata by id
#[cfg(feature = "json-schema")]
pub fn find_step_type(id: &str) -> Option<&'static StepTypeMeta> {
    get_all_step_types().find(|m| m.id == id)
}

// ============================================================================
// Connection Type Metadata (for connection form generation)
// ============================================================================

// ============================================================================
// Connection Enums
// ============================================================================

/// Canonical list of connection categories.
///
/// Used for grouping connection types in the UI and API responses.
/// When adding a new integration, pick the most specific category that fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionCategory {
    Ecommerce,
    FileStorage,
    Llm,
    Crm,
    Erp,
    Database,
    Email,
    Messaging,
    Payment,
    Cloud,
    Api,
}

impl ConnectionCategory {
    /// All categories in preferred display order
    pub const ALL: &[ConnectionCategory] = &[
        ConnectionCategory::Ecommerce,
        ConnectionCategory::FileStorage,
        ConnectionCategory::Llm,
        ConnectionCategory::Crm,
        ConnectionCategory::Erp,
        ConnectionCategory::Database,
        ConnectionCategory::Email,
        ConnectionCategory::Messaging,
        ConnectionCategory::Payment,
        ConnectionCategory::Cloud,
        ConnectionCategory::Api,
    ];

    /// Snake_case identifier
    pub fn id(&self) -> &'static str {
        match self {
            Self::Ecommerce => "ecommerce",
            Self::FileStorage => "file_storage",
            Self::Llm => "llm",
            Self::Crm => "crm",
            Self::Erp => "erp",
            Self::Database => "database",
            Self::Email => "email",
            Self::Messaging => "messaging",
            Self::Payment => "payment",
            Self::Cloud => "cloud",
            Self::Api => "api",
        }
    }

    /// Human-readable display name
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Ecommerce => "E-Commerce",
            Self::FileStorage => "File Storage",
            Self::Llm => "AI / LLM",
            Self::Crm => "CRM",
            Self::Erp => "ERP",
            Self::Database => "Database",
            Self::Email => "Email",
            Self::Messaging => "Messaging",
            Self::Payment => "Payment",
            Self::Cloud => "Cloud",
            Self::Api => "API",
        }
    }

    /// Short description of what this category covers
    pub fn description(&self) -> &'static str {
        match self {
            Self::Ecommerce => "Online store and marketplace platforms",
            Self::FileStorage => "File transfer and cloud storage services",
            Self::Llm => "Large language models and AI services",
            Self::Crm => "Customer relationship management systems",
            Self::Erp => "Enterprise resource planning systems",
            Self::Database => "Relational and document database connections",
            Self::Email => "Email delivery and transactional email services",
            Self::Messaging => "Chat and messaging platforms",
            Self::Payment => "Payment processing and billing platforms",
            Self::Cloud => "Cloud infrastructure providers",
            Self::Api => "Generic REST, GraphQL, or webhook endpoints",
        }
    }

    /// Parse from a string, accepting common legacy variants
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().replace('-', "_").as_str() {
            "ecommerce" | "e_commerce" => Some(Self::Ecommerce),
            "file_storage" | "storage" => Some(Self::FileStorage),
            "llm" | "ai" | "ai_llm" => Some(Self::Llm),
            "crm" => Some(Self::Crm),
            "erp" => Some(Self::Erp),
            "database" | "db" => Some(Self::Database),
            "email" | "smtp" => Some(Self::Email),
            "messaging" | "chat" => Some(Self::Messaging),
            "payment" => Some(Self::Payment),
            "cloud" => Some(Self::Cloud),
            "api" | "http" | "rest" | "graphql" | "webhook" => Some(Self::Api),
            _ => None,
        }
    }
}

impl std::fmt::Display for ConnectionCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id())
    }
}

/// Canonical list of authentication / credential types for connections.
///
/// Describes **what credentials** are used to authenticate, not how they are
/// transported (e.g. bearer header is a delivery mechanism, not a credential type).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionAuthType {
    /// Static secret key for API authentication
    ApiKey,
    /// User-interactive OAuth2 with redirect and consent
    Oauth2AuthorizationCode,
    /// Machine-to-machine OAuth2 token exchange
    Oauth2ClientCredentials,
    /// Credential pair authentication (login + password)
    UsernamePassword,
    /// Private key authentication (e.g. SSH, SFTP)
    SshKey,
    /// IAM-style key pair (key ID + secret key)
    AccessKey,
    /// Database DSN or connection URI
    ConnectionString,
    /// Integration-specific authentication that doesn't fit other types
    Custom,
}

impl ConnectionAuthType {
    /// All auth types in preferred display order
    pub const ALL: &[ConnectionAuthType] = &[
        ConnectionAuthType::ApiKey,
        ConnectionAuthType::Oauth2AuthorizationCode,
        ConnectionAuthType::Oauth2ClientCredentials,
        ConnectionAuthType::UsernamePassword,
        ConnectionAuthType::SshKey,
        ConnectionAuthType::AccessKey,
        ConnectionAuthType::ConnectionString,
        ConnectionAuthType::Custom,
    ];

    /// Snake_case identifier
    pub fn id(&self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::Oauth2AuthorizationCode => "oauth2_authorization_code",
            Self::Oauth2ClientCredentials => "oauth2_client_credentials",
            Self::UsernamePassword => "username_password",
            Self::SshKey => "ssh_key",
            Self::AccessKey => "access_key",
            Self::ConnectionString => "connection_string",
            Self::Custom => "custom",
        }
    }

    /// Human-readable display name
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::ApiKey => "API Key",
            Self::Oauth2AuthorizationCode => "OAuth2 (Authorization Code)",
            Self::Oauth2ClientCredentials => "OAuth2 (Client Credentials)",
            Self::UsernamePassword => "Username & Password",
            Self::SshKey => "SSH Key",
            Self::AccessKey => "Access Key & Secret",
            Self::ConnectionString => "Connection String",
            Self::Custom => "Custom",
        }
    }

    /// Short description of this authentication type
    pub fn description(&self) -> &'static str {
        match self {
            Self::ApiKey => "Static secret key for API authentication",
            Self::Oauth2AuthorizationCode => "User-interactive OAuth2 with redirect and consent",
            Self::Oauth2ClientCredentials => "Machine-to-machine OAuth2 token exchange",
            Self::UsernamePassword => "Credential pair authentication",
            Self::SshKey => "Private key authentication",
            Self::AccessKey => "IAM-style key pair (key ID + secret key)",
            Self::ConnectionString => "Database DSN or connection URI",
            Self::Custom => "Integration-specific authentication",
        }
    }

    /// Parse from a string, accepting legacy SCREAMING_SNAKE_CASE variants
    /// from smo-management and other common forms.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().replace('-', "_").as_str() {
            "api_key" => Some(Self::ApiKey),
            "oauth2_authorization_code" | "oauth2" => Some(Self::Oauth2AuthorizationCode),
            "oauth2_client_credentials" => Some(Self::Oauth2ClientCredentials),
            "username_password" => Some(Self::UsernamePassword),
            "ssh_key" => Some(Self::SshKey),
            "access_key" => Some(Self::AccessKey),
            "connection_string" | "dsn" => Some(Self::ConnectionString),
            "custom" => Some(Self::Custom),
            _ => None,
        }
    }
}

impl std::fmt::Display for ConnectionAuthType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id())
    }
}

// ============================================================================
// Connection Metadata Types
// ============================================================================

/// Factory for a canonical condition attached to connection-form metadata.
///
/// A function pointer keeps the static descriptor const-friendly while letting
/// connection authors construct the same owned [`crate::ConditionExpression`]
/// used by workflows, reports, native validation, and browser WASM. It avoids
/// introducing a second string condition language in the derive macro.
pub type ConnectionConditionFactory = fn() -> crate::ConditionExpression;

/// Canonical conditional state factories emitted by `ConnectionParams`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ConnectionFieldConditions {
    pub visible: Option<ConnectionConditionFactory>,
    pub enabled: Option<ConnectionConditionFactory>,
    pub required: Option<ConnectionConditionFactory>,
}

/// Connection-owned persistence and authorization behavior for one field.
///
/// This intentionally stays outside the shared form model: it governs secret
/// storage and provider authorization lifecycle, not presentation/validation.
#[cfg_attr(feature = "json-schema", derive(schemars::JsonSchema))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ConnectionFieldBehavior {
    /// An existing value may be removed through an explicit patch operation.
    pub clearable: bool,
    /// Changing or clearing this field invalidates captured authorization.
    pub requires_reauthorization: bool,
}

/// Author-owned presentation metadata for a connection form section.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConnectionSectionMeta {
    pub id: &'static str,
    pub label: Option<&'static str>,
    pub description: Option<&'static str>,
    pub order: Option<i32>,
    pub advanced: bool,
}

/// Metadata for a connection field parameter
#[derive(Debug, Clone)]
pub struct ConnectionFieldMeta {
    /// Field name (used in JSON)
    pub name: &'static str,
    /// Type name (String, u16, bool, etc.)
    pub type_name: &'static str,
    /// Whether this field is optional
    pub is_optional: bool,
    /// Display name for UI
    pub display_name: Option<&'static str>,
    /// Description of the field
    pub description: Option<&'static str>,
    /// Placeholder text for the input
    pub placeholder: Option<&'static str>,
    /// Stable form order. The derive macro uses declaration order by default.
    pub order: i32,
    /// Default value as JSON string
    pub default_value: Option<&'static str>,
    /// Whether this is a secret field (password, API key, etc.)
    pub is_secret: bool,
    /// Allowed values when this field is one-of a fixed set. `None` means
    /// free-form input; `Some(&[…])` makes the UI render a select. Values are
    /// the literal strings sent on the wire (e.g. `"none"`, `"bearer"`,
    /// `"api_key"`); labels are derived client-side from the value
    /// (snake_case → Title Case) unless richer metadata is added later.
    pub enum_values: Option<&'static [&'static str]>,
    /// Whether this field must be a syntactically valid absolute https URL when
    /// present. Drives client-side URL validation and the server-side
    /// connection base-URL check that pins credentialed egress to a declared
    /// host. Distinct from `is_optional` (which is structurally derived).
    pub is_url: bool,
    /// Whether this field is *required* (must be present and non-empty),
    /// independent of the structurally-derived `is_optional`. Used to make a
    /// base URL mandatory for credential-bearing generic HTTP connection types.
    pub is_required: bool,
    /// Explicit shared-form control. When absent, the renderer uses canonical
    /// type/enum/secret inference.
    pub control: Option<crate::form::ControlKind>,
    /// Optional shared-form section id.
    pub section: Option<&'static str>,
    /// Whether clients may read and/or write the value.
    pub access: crate::form::FieldAccessMode,
    /// Conditional visible/enabled/required state using canonical expressions.
    pub conditions: ConnectionFieldConditions,
    /// Connection-domain persistence and authorization lifecycle behavior.
    pub behavior: ConnectionFieldBehavior,
}

/// How client credentials are presented to the OAuth token endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TokenEndpointAuth {
    /// `client_id` / `client_secret` in the `application/x-www-form-urlencoded`
    /// body (the OAuth2 default — HubSpot, Google, Salesforce, Microsoft).
    #[default]
    FormBody,
    /// HTTP Basic: `Authorization: Basic base64(client_id:client_secret)`, with the
    /// credentials kept out of the body (required by Intuit/QuickBooks and Xero).
    HttpBasic,
}

/// A provider-specific query parameter returned on the OAuth callback that must be
/// captured into `connection_parameters` (e.g. Intuit returns `realmId`, which is
/// not part of the token response but is needed for every API call).
#[derive(Debug, Clone, Copy)]
pub struct ExtraCallbackParam {
    /// The parameter name as the provider sends it on the callback URL (e.g. `realmId`).
    pub query_name: &'static str,
    /// The key it is stored under in `connection_parameters` (e.g. `realm_id`).
    pub param_name: &'static str,
    /// Whether the callback must fail if this parameter is absent.
    pub required: bool,
}

/// OAuth2 configuration for connection types that use the authorization code flow.
///
/// This is provider-agnostic metadata — the same struct works for HubSpot, Google,
/// Salesforce, QuickBooks, or any OAuth2 provider. The fields below the core
/// endpoints encode the per-provider quirks so the OAuth entry points stay
/// data-driven instead of growing hardcoded per-integration branches.
#[derive(Debug, Clone)]
pub struct OAuthConfig {
    /// Provider's authorization endpoint (e.g., "https://app.hubapi.com/oauth/authorize")
    pub auth_url: &'static str,
    /// Provider's token endpoint (e.g., "https://api.hubapi.com/oauth/v1/token")
    pub token_url: &'static str,
    /// Space-separated default scopes to request
    pub default_scopes: &'static str,
    /// How to present client credentials to the token endpoint.
    pub token_endpoint_auth: TokenEndpointAuth,
    /// Whether the provider rotates (and invalidates) the refresh token on every
    /// refresh. Drives rotation-persistence fail-closed handling.
    pub refresh_token_rotates: bool,
    /// Static API base host, or the production host when `sandbox_base_url` is set.
    /// Empty string = the base URL is not resolved from the descriptor.
    pub base_url: &'static str,
    /// Sandbox API base host, selected when the connection's `environment` param is
    /// `"sandbox"`. Empty string = no sandbox variant.
    pub sandbox_base_url: &'static str,
    /// Optional path appended after the host, with `{param}` placeholders substituted
    /// from connection parameters (e.g. `/v3/company/{realm_id}`). Empty = none.
    pub base_url_path_template: &'static str,
    /// Provider-specific callback query params to capture into `connection_parameters`.
    pub extra_callback_params: &'static [ExtraCallbackParam],
    /// OAuth `error` codes on the token endpoint that mean the grant is dead and the
    /// user must re-authorize (e.g. `invalid_grant`). When a refresh fails with one of
    /// these, the connection is flipped to `REQUIRES_RECONNECTION` instead of retried
    /// forever. Empty = never auto-flip.
    pub reauth_on_error_codes: &'static [&'static str],
    /// Provider token-revocation endpoint, called on disconnect to invalidate the
    /// tokens provider-side. Empty = no revocation.
    pub revocation_endpoint: &'static str,
    /// Whether the authorization-code flow must use PKCE (RFC 7636). When true, the
    /// authorize URL carries an S256 `code_challenge` and the exchange sends the
    /// `code_verifier`.
    pub pkce_required: bool,
    /// THE per-type overlay gate: when true (ONLY the generic bring-your-own
    /// types), OAuth endpoints/config are read from connection parameters.
    /// When false (every curated provider), parameters are ignored for ALL
    /// OAuth config fields — including ones the descriptor legitimately leaves
    /// empty — so a params PATCH can never redirect a curated provider's
    /// credentialed egress.
    pub params_driven: bool,
}

/// Metadata for a connection type.
#[derive(Debug, Clone)]
pub struct ConnectionTypeMeta {
    /// Unique identifier for this connection type (e.g., "bearer", "sftp")
    pub integration_id: &'static str,
    /// Display name for UI (e.g., "Bearer Token", "SFTP")
    pub display_name: &'static str,
    /// Description of this connection type
    pub description: Option<&'static str>,
    /// Category for grouping (e.g., Ecommerce, FileStorage, Llm)
    pub category: Option<ConnectionCategory>,
    /// External service identifier (e.g., "shopify", "openai")
    pub service_id: Option<&'static str>,
    /// Authentication / credential type
    pub auth_type: Option<ConnectionAuthType>,
    /// Fields required for this connection type
    pub fields: &'static [ConnectionFieldMeta],
    /// Explicit section presentation metadata authored by the descriptor.
    pub sections: &'static [ConnectionSectionMeta],
    /// OAuth2 configuration (only for auth_type = Oauth2AuthorizationCode)
    pub oauth_config: Option<&'static OAuthConfig>,
}

/// Get all registered connection type metadata
pub fn get_all_connection_types() -> std::iter::Empty<&'static ConnectionTypeMeta> {
    std::iter::empty()
}

/// Find connection type metadata by integration_id
pub fn find_connection_type(integration_id: &str) -> Option<&'static ConnectionTypeMeta> {
    get_all_connection_types().find(|m| m.integration_id == integration_id)
}

// ============================================================================
// Conversion Functions (static metadata -> API types)
// ============================================================================

/// Type conversion result with optional schema
struct TypeConversionResult {
    json_type: String,
    format: Option<String>,
    items_json: Option<String>,
    schema: Option<serde_json::Value>,
}

/// Convert Rust type to JSON Schema type with optional schema hints
fn rust_to_json_schema_type_with_schema(rust_type: &str) -> TypeConversionResult {
    match rust_type {
        "String" => TypeConversionResult {
            json_type: "string".to_string(),
            format: None,
            items_json: None,
            schema: None,
        },
        "bool" => TypeConversionResult {
            json_type: "boolean".to_string(),
            format: None,
            items_json: None,
            schema: None,
        },
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
        | "usize" => TypeConversionResult {
            json_type: "integer".to_string(),
            format: None,
            items_json: None,
            schema: None,
        },
        "f32" | "f64" => TypeConversionResult {
            json_type: "number".to_string(),
            format: Some("double".to_string()),
            items_json: None,
            schema: None,
        },
        "Value" => TypeConversionResult {
            json_type: "any".to_string(),
            format: None,
            items_json: None,
            schema: None,
        },
        "()" => TypeConversionResult {
            json_type: "null".to_string(),
            format: None,
            items_json: None,
            schema: None,
        },
        "ConditionExpression" => TypeConversionResult {
            json_type: "object".to_string(),
            format: None,
            items_json: None,
            schema: Some(get_condition_expression_schema()),
        },
        // Option<T> only affects nullability (tracked separately by the macro);
        // the JSON type is the inner type's. Without this arm every Option<i64>
        // fell through the catch-all and serialized as "string".
        // strip_prefix/strip_suffix strip exactly one layer — trim_start_matches
        // would eat repeated prefixes and turn Vec<Vec<Value>> inner into "Value".
        t if t.starts_with("Option<") => {
            let inner = t
                .strip_prefix("Option<")
                .and_then(|s| s.strip_suffix('>'))
                .unwrap_or("");
            rust_to_json_schema_type_with_schema(inner)
        }
        t if t.starts_with("Vec<") => {
            let inner = t
                .strip_prefix("Vec<")
                .and_then(|s| s.strip_suffix('>'))
                .unwrap_or("");
            let inner_result = rust_to_json_schema_type_with_schema(inner);
            let items_json = if let Some(fmt) = inner_result.format {
                format!(
                    r#"{{"type": "{}", "format": "{}"}}"#,
                    inner_result.json_type, fmt
                )
            } else {
                format!(r#"{{"type": "{}"}}"#, inner_result.json_type)
            };
            TypeConversionResult {
                json_type: "array".to_string(),
                format: None,
                items_json: Some(items_json),
                schema: None,
            }
        }
        t if t.starts_with("HashMap<") || t.starts_with("BTreeMap<") => TypeConversionResult {
            json_type: "object".to_string(),
            format: None,
            items_json: None,
            schema: None,
        },
        _ => TypeConversionResult {
            json_type: "string".to_string(),
            format: None,
            items_json: None,
            schema: None,
        },
    }
}

/// Legacy function for backwards compatibility
fn rust_to_json_schema_type(rust_type: &str) -> (String, Option<String>, Option<String>) {
    let result = rust_to_json_schema_type_with_schema(rust_type);
    (result.json_type, result.format, result.items_json)
}

/// Get JSON Schema for ConditionExpression type.
///
/// Uses schemars when the `json-schema` feature is on; otherwise (or if
/// schemars serialization fails) returns the hand-written fallback so
/// callers in no-`json-schema` builds (e.g. WASM) still get a usable
/// schema description.
fn get_condition_expression_schema() -> serde_json::Value {
    #[cfg(feature = "json-schema")]
    {
        let schema = schemars::schema_for!(crate::ConditionExpression);
        if let Ok(value) = serde_json::to_value(schema) {
            return value;
        }
    }
    condition_expression_fallback_schema()
}

fn condition_expression_fallback_schema() -> serde_json::Value {
    serde_json::json!({
        "oneOf": [
            {
                "type": "object",
                "title": "Operation",
                "description": "A comparison or logical operation",
                "properties": {
                    "type": { "const": "operation" },
                    "op": {
                        "type": "string",
                        "enum": ["And", "Or", "Not", "Eq", "Ne", "Gt", "Lt", "Gte", "Lte",
                                 "StartsWith", "EndsWith", "Contains", "In", "NotIn",
                                 "Length", "IsDefined", "IsEmpty", "IsNotEmpty"]
                    },
                    "arguments": {
                        "type": "array",
                        "description": "Arguments can be nested expressions or values (reference/immediate)"
                    }
                },
                "required": ["type", "op", "arguments"]
            },
            {
                "type": "object",
                "title": "Value",
                "description": "A direct value (reference or immediate) - evaluated as truthy/falsy",
                "properties": {
                    "type": { "const": "value" },
                    "valueType": { "type": "string", "enum": ["reference", "immediate"] },
                    "value": { "description": "The value content" }
                },
                "required": ["type", "valueType", "value"]
            }
        ]
    })
}

/// Convert InputFieldMeta to CapabilityField
pub fn input_field_to_api(field: &InputFieldMeta) -> CapabilityField {
    let type_result = rust_to_json_schema_type_with_schema(field.type_name);

    let items = type_result.items_json.map(|items_str| {
        // Parse items JSON to extract type and format
        let type_match = items_str
            .split("\"type\": \"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .unwrap_or("string");
        let format_match = if items_str.contains("\"format\"") {
            items_str
                .split("\"format\": \"")
                .nth(1)
                .and_then(|s| s.split('"').next())
                .map(|s| s.to_string())
        } else {
            None
        };
        FieldTypeInfo {
            type_name: type_match.to_string(),
            format: format_match,
            display_name: None,
            description: None,
            fields: None,
            items: None,
            nullable: false,
        }
    });

    let default_value = field
        .default_value
        .and_then(|s| serde_json::from_str(s).ok());

    let example = field
        .example
        .map(|s| serde_json::Value::String(s.to_string()));

    let enum_values = field
        .enum_values_fn
        .map(|f| f().iter().map(|s| s.to_string()).collect());

    CapabilityField {
        name: field.name.to_string(),
        display_name: field.display_name.map(|s| s.to_string()),
        description: field.description.map(|s| s.to_string()),
        type_name: type_result.json_type,
        format: type_result.format,
        items,
        required: !field.is_optional,
        default_value,
        example,
        enum_values,
        schema: type_result.schema,
    }
}

/// Depth cap for resolving nested output types. The visited-set below is the
/// primary cycle guard; this bounds pathological (deep but acyclic) chains so
/// the serialized meta.json stays reasonable.
const MAX_OUTPUT_TYPE_DEPTH: usize = 8;

/// Registry of output struct metadata keyed by type name, used to inline
/// nested/item struct fields into the serialized schema. Agents build this
/// from their macro-emitted `__OUTPUT_META_*` statics.
pub type OutputTypeRegistry<'a> = std::collections::HashMap<&'a str, &'a OutputTypeMeta>;

fn can_descend(type_name: &str, visited: &[String]) -> bool {
    visited.len() < MAX_OUTPUT_TYPE_DEPTH && !visited.iter().any(|v| v == type_name)
}

/// A bare custom type name ("HttpResponseBody"), as opposed to a primitive,
/// `String`/`Value`, or a generic like `Vec<...>`.
fn is_custom_type_name(name: &str) -> bool {
    name != "String"
        && name != "Value"
        && !name.contains('<')
        && name
            .chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false)
}

fn resolve_output_fields(
    meta: &OutputTypeMeta,
    output_types: &OutputTypeRegistry<'_>,
    visited: &mut Vec<String>,
) -> Vec<OutputField> {
    meta.fields
        .iter()
        .map(|field| resolve_output_field(field, output_types, visited))
        .collect()
}

fn resolve_output_field(
    field: &OutputFieldMeta,
    output_types: &OutputTypeRegistry<'_>,
    visited: &mut Vec<String>,
) -> OutputField {
    let (mut type_name, format, items_json) = rust_to_json_schema_type(field.type_name);
    let mut fields: Option<Box<Vec<OutputField>>> = None;
    let mut items: Option<Box<FieldTypeInfo>> = None;

    if let Some(nested) = field.nested_type_name {
        if let Some(meta) = output_types.get(nested) {
            // Registered type: definitely a struct — resolve its fields inline.
            type_name = "object".to_string();
            if can_descend(nested, visited) {
                visited.push(nested.to_string());
                fields = Some(Box::new(resolve_output_fields(meta, output_types, visited)));
                visited.pop();
            }
        } else {
            // Unregistered custom type: could be a struct, a unit enum (string
            // on the wire), or a data enum like http's HttpResponseBody whose
            // wire form varies per response. The mapper's catch-all "string"
            // would be a confident lie the editor now surfaces as badges and
            // mismatch warnings — declare the type unknown instead.
            type_name = "any".to_string();
        }
    } else if let Some(item_type) = field.items_type_name {
        // Vec<T>: the macro records T for every Vec, primitive or struct.
        if let Some(meta) = output_types.get(item_type) {
            type_name = "array".to_string();
            let mut item_fields = None;
            if can_descend(item_type, visited) {
                visited.push(item_type.to_string());
                item_fields = Some(Box::new(resolve_output_fields(meta, output_types, visited)));
                visited.pop();
            }
            items = Some(Box::new(FieldTypeInfo {
                type_name: "object".to_string(),
                format: None,
                display_name: meta.display_name.map(|s| s.to_string()),
                description: meta.description.map(|s| s.to_string()),
                fields: item_fields,
                items: None,
                nullable: false,
            }));
        } else if is_custom_type_name(item_type) {
            // Vec of an unregistered custom type: the elements' wire shape is
            // unknown — say so instead of the mapper's catch-all "string".
            type_name = "array".to_string();
            items = Some(Box::new(FieldTypeInfo {
                type_name: "any".to_string(),
                format: None,
                display_name: None,
                description: None,
                fields: None,
                items: None,
                nullable: false,
            }));
        }
        // Primitive item types keep the type mapper's items_json result below.
    }

    // Vec<primitive>: surface the item type computed by the type mapper.
    if items.is_none()
        && let Some(items_str) = items_json
    {
        items = serde_json::from_str::<FieldTypeInfo>(&items_str)
            .ok()
            .map(Box::new);
    }

    OutputField {
        name: field.name.to_string(),
        display_name: field.display_name.map(|s| s.to_string()),
        description: field.description.map(|s| s.to_string()),
        type_name,
        format,
        example: field
            .example
            .map(|s| serde_json::Value::String(s.to_string())),
        nullable: field.nullable,
        items,
        fields,
    }
}

/// Convert OutputFieldMeta to OutputField, resolving nested struct types
/// through the registry so the serialized schema carries the full shape.
pub fn output_field_to_api_with_types(
    field: &OutputFieldMeta,
    output_types: &OutputTypeRegistry<'_>,
) -> OutputField {
    resolve_output_field(field, output_types, &mut Vec::new())
}

/// Convert OutputFieldMeta to OutputField without a type registry: nested
/// struct fields stay unresolved (shape unknown), but scalar types are exact.
pub fn output_field_to_api(field: &OutputFieldMeta) -> OutputField {
    output_field_to_api_with_types(field, &OutputTypeRegistry::new())
}

/// Convert CapabilityMeta to CapabilityInfo without a nested-type registry.
/// Prefer [`capability_to_api_with_types`]: without the registry, nested
/// struct fields inside the output stay unresolved.
pub fn capability_to_api(
    cap: &CapabilityMeta,
    input_type_meta: Option<&InputTypeMeta>,
    output_type_meta: Option<&OutputTypeMeta>,
) -> CapabilityInfo {
    capability_to_api_with_types(
        cap,
        input_type_meta,
        output_type_meta,
        &OutputTypeRegistry::new(),
    )
}

/// Convert CapabilityMeta to CapabilityInfo, resolving nested output struct
/// types through the registry so meta.json carries the full recursive shape.
pub fn capability_to_api_with_types(
    cap: &CapabilityMeta,
    input_type_meta: Option<&InputTypeMeta>,
    output_type_meta: Option<&OutputTypeMeta>,
    output_types: &OutputTypeRegistry<'_>,
) -> CapabilityInfo {
    let (mut output_type, output_format, output_items_json) =
        rust_to_json_schema_type(cap.output_type);
    let mut output_items: Option<Box<FieldTypeInfo>> = None;

    let inputs = input_type_meta
        .map(|m| m.fields.iter().map(input_field_to_api).collect())
        .unwrap_or_default();

    let output_fields = output_type_meta.map(|m| {
        Box::new(resolve_output_fields(
            m,
            output_types,
            &mut vec![m.type_name.to_string()],
        ))
    });

    if output_type_meta.is_some() {
        // A registered output struct is an object; the type mapper's catch-all
        // would have reported the struct's Rust name as "string".
        output_type = "object".to_string();
    } else if output_type == "array" {
        // Vec<T> capability output: resolve the item struct when registered.
        let inner = cap
            .output_type
            .strip_prefix("Vec<")
            .and_then(|s| s.strip_suffix('>'))
            .unwrap_or("");
        if let Some(meta) = output_types.get(inner) {
            output_items = Some(Box::new(FieldTypeInfo {
                type_name: "object".to_string(),
                format: None,
                display_name: meta.display_name.map(|s| s.to_string()),
                description: meta.description.map(|s| s.to_string()),
                fields: Some(Box::new(resolve_output_fields(
                    meta,
                    output_types,
                    &mut vec![meta.type_name.to_string()],
                ))),
                items: None,
                nullable: false,
            }));
        } else if is_custom_type_name(inner) {
            // Vec of an unregistered custom type: element shape unknown.
            output_items = Some(Box::new(FieldTypeInfo {
                type_name: "any".to_string(),
                format: None,
                display_name: None,
                description: None,
                fields: None,
                items: None,
                nullable: false,
            }));
        } else if let Some(items_str) = output_items_json {
            output_items = serde_json::from_str::<FieldTypeInfo>(&items_str)
                .ok()
                .map(Box::new);
        }
    } else if output_type == "string" && is_custom_type_name(cap.output_type) {
        // Unregistered custom output type (e.g. shopify's CommerceProduct):
        // could be a struct or an enum — the mapper's catch-all "string" is a
        // guess the editor would present as authoritative. Declare it unknown;
        // registering the struct is the real fix.
        output_type = "any".to_string();
    }

    // Convert compensation hint if present
    let compensation_hint = cap
        .compensation_hint
        .as_ref()
        .map(|h| CompensationHintInfo {
            capability_id: h.capability_id.to_string(),
            description: h.description.map(|s| s.to_string()),
        });

    // Convert known errors
    let known_errors: Vec<KnownErrorInfo> =
        cap.known_errors.iter().map(KnownErrorInfo::from).collect();

    CapabilityInfo {
        id: cap.capability_id.to_string(),
        name: cap.function_name.to_string(),
        display_name: cap.display_name.map(|s| s.to_string()),
        description: cap.description.map(|s| s.to_string()),
        input_type: cap.input_type.to_string(),
        inputs,
        output: FieldTypeInfo {
            type_name: output_type,
            format: output_format,
            display_name: output_type_meta.and_then(|m| m.display_name.map(|s| s.to_string())),
            description: output_type_meta.and_then(|m| m.description.map(|s| s.to_string())),
            fields: output_fields,
            items: output_items,
            nullable: false,
        },
        has_side_effects: cap.has_side_effects,
        is_idempotent: cap.is_idempotent,
        rate_limited: cap.rate_limited,
        compensation_hint,
        known_errors,
        tags: cap.tags.iter().map(|s| s.to_string()).collect(),
    }
}

/// Build API-compatible agent list from this crate's local metadata.
pub fn get_agents() -> Vec<AgentInfo> {
    use std::collections::HashMap;

    // Collect all input types into a map for lookup
    let input_types: HashMap<&str, &InputTypeMeta> =
        get_all_input_types().map(|m| (m.type_name, m)).collect();

    // Collect all output types into a map for lookup
    let output_types: HashMap<&str, &OutputTypeMeta> =
        get_all_output_types().map(|m| (m.type_name, m)).collect();

    // Group capabilities by module. Keyed canonically — capability macros
    // may declare `module = "object_model"` while the module config id is
    // kebab; both must land in the same bucket.
    let mut caps_by_module: HashMap<String, Vec<&CapabilityMeta>> = HashMap::new();
    for cap in get_all_capabilities() {
        let module = canonical_agent_id(cap.module.unwrap_or("unknown"));
        caps_by_module.entry(module).or_default().push(cap);
    }

    // Build agent info for each module
    let mut agents = Vec::new();

    for config in get_all_agent_modules() {
        let caps = caps_by_module
            .get(&canonical_agent_id(config.id))
            .cloned()
            .unwrap_or_default();

        if caps.is_empty() {
            continue;
        }

        let capabilities: Vec<CapabilityInfo> = caps
            .iter()
            .map(|cap| {
                let input_meta = input_types.get(cap.input_type).copied();
                let output_meta = output_types.get(cap.output_type).copied();
                capability_to_api_with_types(cap, input_meta, output_meta, &output_types)
            })
            .collect();

        agents.push(AgentInfo {
            id: config.id.to_string(),
            name: config.name.to_string(),
            description: config.description.to_string(),
            has_side_effects: config.has_side_effects,
            supports_connections: config.supports_connections,
            integration_ids: config
                .integration_ids
                .iter()
                .map(|s| s.to_string())
                .collect(),
            capabilities,
        });
    }

    agents
}

/// Get input field definitions for a specific capability.
/// Returns None if the agent or capability is not found.
pub fn get_capability_inputs(agent_id: &str, capability_id: &str) -> Option<Vec<CapabilityField>> {
    use std::collections::HashMap;

    // Collect all input types into a map for lookup
    let input_types: HashMap<&str, &InputTypeMeta> =
        get_all_input_types().map(|m| (m.type_name, m)).collect();

    // Find the capability (case-insensitive module match)
    let agent_lower = agent_id.to_lowercase();
    for cap in get_all_capabilities() {
        let module = cap.module.unwrap_or("unknown");
        if module == agent_lower && cap.capability_id == capability_id {
            // Get input type metadata
            let input_meta = input_types.get(cap.input_type).copied();

            // Build the inputs list using the existing conversion function
            let inputs: Vec<CapabilityField> = if let Some(meta) = input_meta {
                meta.fields.iter().map(input_field_to_api).collect()
            } else {
                Vec::new()
            };

            return Some(inputs);
        }
    }

    None
}

// ============================================================================
// Validation Functions
// ============================================================================

/// Primitive types that don't require CapabilityOutput registration
const PRIMITIVE_OUTPUT_TYPES: &[&str] = &[
    "()",   // Unit type
    "bool", // Boolean
    "i8",
    "i16",
    "i32",
    "i64",
    "i128",
    "isize", // Signed integers
    "u8",
    "u16",
    "u32",
    "u64",
    "u128",
    "usize", // Unsigned integers
    "f32",
    "f64",               // Floats
    "String",            // String
    "Value",             // serde_json::Value - dynamic JSON (short form)
    "serde_json::Value", // serde_json::Value - dynamic JSON (fully qualified)
];

/// Check if a type is a primitive that doesn't need CapabilityOutput
fn is_primitive_output_type(type_name: &str) -> bool {
    // Check exact matches
    if PRIMITIVE_OUTPUT_TYPES.contains(&type_name) {
        return true;
    }

    // Check Vec<T> where T is primitive
    if let Some(inner) = type_name
        .strip_prefix("Vec<")
        .and_then(|s| s.strip_suffix('>'))
    {
        return is_primitive_output_type(inner);
    }

    // Check Option<T> where T is primitive
    if let Some(inner) = type_name
        .strip_prefix("Option<")
        .and_then(|s| s.strip_suffix('>'))
    {
        return is_primitive_output_type(inner);
    }

    // Check HashMap<K, V> where V is primitive (common for headers, etc.)
    if type_name.starts_with("HashMap<") || type_name.starts_with("BTreeMap<") {
        return true; // Allow map types as they're typically dynamic
    }

    false
}

/// Validation error for missing agent metadata
#[derive(Debug, Clone)]
pub struct AgentValidationError {
    pub module: String,
    pub capability_id: String,
    pub missing_input: bool,
    pub missing_output: bool,
    pub input_type: String,
    pub output_type: String,
}

impl std::fmt::Display for AgentValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut issues = Vec::new();
        if self.missing_input {
            issues.push(format!("missing CapabilityInput for '{}'", self.input_type));
        }
        if self.missing_output {
            issues.push(format!(
                "missing CapabilityOutput for '{}'",
                self.output_type
            ));
        }
        write!(
            f,
            "{}:{} - {}",
            self.module,
            self.capability_id,
            issues.join(", ")
        )
    }
}

/// Validate that all registered capabilities have corresponding input and output metadata.
///
/// Returns a list of validation errors. If the list is empty, all capabilities are valid.
///
/// # Example
/// ```ignore
/// let errors = validate_agent_metadata();
/// if !errors.is_empty() {
///     for error in &errors {
///         eprintln!("Agent validation error: {}", error);
///     }
///     panic!("Agent metadata validation failed");
/// }
/// ```
pub fn validate_agent_metadata() -> Vec<AgentValidationError> {
    use std::collections::HashMap;

    let input_types: HashMap<&str, &InputTypeMeta> =
        get_all_input_types().map(|m| (m.type_name, m)).collect();

    let output_types: HashMap<&str, &OutputTypeMeta> =
        get_all_output_types().map(|m| (m.type_name, m)).collect();

    // Helper to check if output type is valid (primitive, registered, or Vec/Option of registered)
    let is_valid_output = |type_name: &str| -> bool {
        if is_primitive_output_type(type_name) {
            return true;
        }
        if output_types.contains_key(type_name) {
            return true;
        }
        // Check Vec<T> where T is a registered output type
        if let Some(inner) = type_name
            .strip_prefix("Vec<")
            .and_then(|s| s.strip_suffix('>'))
        {
            return is_primitive_output_type(inner) || output_types.contains_key(inner);
        }
        // Check Option<T> where T is a registered output type
        if let Some(inner) = type_name
            .strip_prefix("Option<")
            .and_then(|s| s.strip_suffix('>'))
        {
            return is_primitive_output_type(inner) || output_types.contains_key(inner);
        }
        false
    };

    let mut errors = Vec::new();

    for cap in get_all_capabilities() {
        let module = cap.module.unwrap_or("unknown").to_string();
        let missing_input = !input_types.contains_key(cap.input_type);
        let missing_output = !is_valid_output(cap.output_type);

        if missing_input || missing_output {
            errors.push(AgentValidationError {
                module,
                capability_id: cap.capability_id.to_string(),
                missing_input,
                missing_output,
                input_type: cap.input_type.to_string(),
                output_type: cap.output_type.to_string(),
            });
        }
    }

    errors
}

/// Validate agent metadata and panic if any capabilities are missing input/output definitions.
///
/// Call this at application startup to ensure all agents are properly defined.
///
/// # Panics
/// Panics with a detailed error message listing all capabilities with missing metadata.
pub fn validate_agent_metadata_or_panic() {
    let errors = validate_agent_metadata();
    if !errors.is_empty() {
        let error_list: Vec<String> = errors.iter().map(|e| format!("  - {}", e)).collect();
        panic!(
            "Agent metadata validation failed!\n\
             The following capabilities are missing CapabilityInput or CapabilityOutput definitions:\n\
             {}\n\n\
             To fix this:\n\
             1. For input types: Add #[derive(CapabilityInput)] to the input struct\n\
             2. For output types: Add #[derive(CapabilityOutput)] to the output struct\n\
             \n\
             Example:\n\
             #[derive(Serialize, Deserialize, CapabilityOutput)]\n\
             #[capability_output(display_name = \"My Output\")]\n\
             pub struct MyCapabilityOutput {{\n\
                 #[field(display_name = \"Result\", description = \"The capability result\")]\n\
                 pub result: String,\n\
             }}",
            error_list.join("\n")
        );
    }
}

// ============================================================================
// AgentCatalog — runtime-loaded snapshot of every agent's metadata.
// ============================================================================

/// Canonicalize an agent id to its kebab-case form.
///
/// Kebab-case is the canonical agent id everywhere it matters at runtime: the
/// component dispatcher forces each `meta.json` `id` to kebab, `GET
/// /api/runtime/agents` advertises kebab, and the WASM component packages are
/// named `runtara:agent-<kebab>`. Workflow JSON authored against an older
/// snake_case id (`object_model`) — or with stray capitalization — folds to
/// the same canonical id, so catalog lookups and id-specific validation rules
/// resolve identically regardless of which form the author used. The compile
/// path (`direct_wasm`) and the server's agent-discovery service already fold
/// the same way; this is the shared definition they all agree on.
pub fn canonical_agent_id(id: &str) -> String {
    id.to_ascii_lowercase().replace('_', "-")
}

/// Maximum length of a workflow slug (the capability id of a
/// workflow-as-agent). WIT imposes no cap; this protects the
/// `runtara_agent_<snake>.wasm` staging filenames and keeps ids readable.
pub const WORKFLOW_SLUG_MAX_LEN: usize = 64;

/// Why a user-supplied workflow slug was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlugError {
    /// The slug is empty.
    Empty,
    /// The slug exceeds [`WORKFLOW_SLUG_MAX_LEN`].
    TooLong { len: usize },
    /// The slug contains a character outside `[a-z0-9-]`.
    InvalidChar(char),
    /// The slug has a leading/trailing hyphen or a `--` run (every
    /// hyphen-separated part must be non-empty — wit-parser's rule).
    EdgeOrDoubleHyphen,
}

impl std::fmt::Display for SlugError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "slug must not be empty"),
            Self::TooLong { len } => write!(
                f,
                "slug is {len} characters; the maximum is {WORKFLOW_SLUG_MAX_LEN}"
            ),
            Self::InvalidChar(ch) => write!(
                f,
                "slug may only contain lowercase letters, digits, and hyphens (found {ch:?})"
            ),
            Self::EdgeOrDoubleHyphen => write!(
                f,
                "slug must not start or end with a hyphen or contain consecutive hyphens"
            ),
        }
    }
}

impl std::error::Error for SlugError {}

/// Derive a workflow's slug from its name — the WIT-safe capability id a
/// workflow-as-agent exports as `runtara:agent-<slug>/capabilities`.
///
/// Transform: lowercase; every run of characters outside `[a-z0-9]` collapses
/// to a single `-`; edge hyphens trimmed; capped at
/// [`WORKFLOW_SLUG_MAX_LEN`] (re-trimming a hyphen exposed by truncation).
/// An un-nameable result falls back to `wf-<first 8 hex of workflow_id>`.
///
/// Leading digits are allowed (`2fa-sync` is a valid slug): wit-parser
/// accepts digit-led words in package names — asserted by a test in
/// `runtara-workflows` against the real parser. The output is always
/// idempotent under [`canonical_agent_id`] and passes
/// [`validate_workflow_slug`].
pub fn generate_workflow_slug(name: &str, workflow_id: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    let mut pending_hyphen = false;
    for ch in name.chars() {
        let ch = ch.to_ascii_lowercase();
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            if pending_hyphen && !slug.is_empty() {
                slug.push('-');
            }
            slug.push(ch);
            pending_hyphen = false;
        } else {
            pending_hyphen = true;
        }
    }

    if slug.len() > WORKFLOW_SLUG_MAX_LEN {
        // Only ASCII `[a-z0-9-]` ever reaches here, so byte truncation is safe.
        slug.truncate(WORKFLOW_SLUG_MAX_LEN);
        while slug.ends_with('-') {
            slug.pop();
        }
    }

    if slug.is_empty() {
        let hex: String = workflow_id
            .chars()
            .filter(char::is_ascii_hexdigit)
            .take(8)
            .collect::<String>()
            .to_ascii_lowercase();
        slug = if hex.is_empty() {
            "wf".to_string()
        } else {
            format!("wf-{hex}")
        };
    }

    slug
}

/// Validate a user-supplied workflow slug (author/edit-time gate).
///
/// Rules: non-empty; at most [`WORKFLOW_SLUG_MAX_LEN`] characters; only
/// `[a-z0-9-]`; no leading/trailing hyphen and no `--` run (each
/// hyphen-separated part non-empty — exactly wit-parser's rule, so
/// `agent-<slug>` is a valid WIT package name and
/// `canonical_agent_id(slug) == slug`). Leading digits are allowed.
pub fn validate_workflow_slug(slug: &str) -> Result<(), SlugError> {
    if slug.is_empty() {
        return Err(SlugError::Empty);
    }
    if slug.len() > WORKFLOW_SLUG_MAX_LEN {
        return Err(SlugError::TooLong { len: slug.len() });
    }
    if let Some(ch) = slug
        .chars()
        .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-'))
    {
        return Err(SlugError::InvalidChar(ch));
    }
    if slug.starts_with('-') || slug.ends_with('-') || slug.contains("--") {
        return Err(SlugError::EdgeOrDoubleHyphen);
    }
    Ok(())
}

/// Capability id of a workflow-as-agent's single capability. The agent id is
/// the workflow's slug; the one capability that runs the workflow is `run`
/// (an Agent step targets it as `agentId: <slug>, capabilityId: "run"`).
pub const WORKFLOW_AGENT_CAPABILITY_ID: &str = "run";

/// Synthesize the catalog metadata for a workflow published as an agent — the
/// exact `AgentInfo` shape a native agent's `meta.json` sidecar carries, so a
/// staged workflow-agent drops into the same catalog/validation/step-picker
/// machinery with zero special-casing.
///
/// The capability's `inputs` ARE the workflow's `inputSchema` fields —
/// including `connection`-typed ones (surfaced with `type: "connection"`),
/// which is how a workflow-agent advertises the connections a CALLER must
/// supply: there is no separate connection-slots descriptor
/// (docs/workflow-agent-connections.md). `supportsConnections` and
/// `integrationIds` are derived from those fields.
///
/// `hasSideEffects` is conservatively `true` (a workflow may do anything) and
/// `isIdempotent` `false` — callers must not silently retry a published
/// workflow.
pub fn workflow_agent_info(
    slug: &str,
    name: &str,
    description: &str,
    input_schema: &std::collections::HashMap<String, crate::SchemaField>,
    output_schema: &std::collections::HashMap<String, crate::SchemaField>,
) -> AgentInfo {
    let mut inputs: Vec<CapabilityField> = input_schema
        .iter()
        .map(|(field_name, field)| capability_field_from_schema(field_name, field))
        .collect();
    // HashMap iteration is nondeterministic; the meta.json must be stable
    // across recompiles (checksummed sidecars, diffs).
    inputs.sort_by(|a, b| a.name.cmp(&b.name));

    let mut output_fields: Vec<OutputField> = output_schema
        .iter()
        .map(|(field_name, field)| OutputField {
            name: field_name.clone(),
            display_name: field.label.clone(),
            description: field.description.clone(),
            type_name: field.field_type.as_str().to_string(),
            format: field.format.clone(),
            example: field.example.clone(),
            nullable: field.nullable.unwrap_or(false),
            items: field
                .items
                .as_ref()
                .map(|item| Box::new(field_type_info_from_schema(item))),
            fields: None,
        })
        .collect();
    output_fields.sort_by(|a, b| a.name.cmp(&b.name));

    let integration_ids: Vec<String> = {
        let mut ids: Vec<String> = input_schema
            .values()
            .filter(|field| matches!(field.field_type, crate::SchemaFieldType::Connection))
            .filter_map(|field| field.integration.clone())
            .collect();
        ids.sort();
        ids.dedup();
        ids
    };

    AgentInfo {
        id: slug.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        has_side_effects: true,
        // `supportsConnections` means "the agent takes a STEP-LEVEL connection"
        // (validators then require one on every step targeting it). A
        // workflow-agent never does — its connections are ordinary
        // `connection`-typed INPUTS, enforced through required-input
        // validation like any other field. `integrationIds` stays populated
        // (informational: which integrations those inputs want).
        supports_connections: false,
        integration_ids,
        capabilities: vec![CapabilityInfo {
            id: WORKFLOW_AGENT_CAPABILITY_ID.to_string(),
            name: WORKFLOW_AGENT_CAPABILITY_ID.to_string(),
            display_name: Some(format!("Run {name}")),
            description: Some(if description.is_empty() {
                format!("Run the '{name}' workflow as an agent capability")
            } else {
                description.to_string()
            }),
            input_type: "WorkflowInput".to_string(),
            inputs,
            output: FieldTypeInfo {
                type_name: "object".to_string(),
                format: None,
                display_name: None,
                description: None,
                fields: if output_fields.is_empty() {
                    None
                } else {
                    Some(Box::new(output_fields))
                },
                items: None,
                nullable: false,
            },
            has_side_effects: true,
            is_idempotent: false,
            rate_limited: false,
            compensation_hint: None,
            known_errors: Vec::new(),
            tags: vec!["workflow-agent".to_string()],
        }],
    }
}

fn capability_field_from_schema(field_name: &str, field: &crate::SchemaField) -> CapabilityField {
    CapabilityField {
        name: field_name.to_string(),
        display_name: field.label.clone(),
        description: field.description.clone(),
        type_name: field.field_type.as_str().to_string(),
        format: field.format.clone(),
        items: field
            .items
            .as_ref()
            .map(|item| field_type_info_from_schema(item)),
        required: field.required,
        default_value: field.default.clone(),
        example: field.example.clone(),
        enum_values: field.enum_values.as_ref().map(|values| {
            values
                .iter()
                .map(|value| match value {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect()
        }),
        schema: None,
    }
}

fn field_type_info_from_schema(field: &crate::SchemaField) -> FieldTypeInfo {
    FieldTypeInfo {
        type_name: field.field_type.as_str().to_string(),
        format: field.format.clone(),
        display_name: field.label.clone(),
        description: field.description.clone(),
        fields: None,
        items: field
            .items
            .as_ref()
            .map(|item| Box::new(field_type_info_from_schema(item))),
        nullable: field.nullable.unwrap_or(false),
    }
}

/// A snapshot of every agent the runtime knows about, indexed by id.
///
/// The catalog is the runtime replacement for `runtara-agents::static_registry`
/// — instead of compile-time arrays baked into the server/validator binary,
/// it's populated at startup from the `<agent>.meta.json` sidecars staged
/// next to each `.wasm` in `$RUNTARA_AGENT_COMPONENTS_DIR`.
///
/// Two loaders:
/// - [`AgentCatalog::from_meta_dir`] — server-side, walks a directory of
///   `runtara_agent_*.meta.json` files.
/// - [`AgentCatalog::from_json`] — browser-side, accepts a JSON array
///   shipped over `GET /api/runtime/agents` so the WASM validator doesn't
///   embed agents at compile time.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AgentCatalog {
    agents: Vec<AgentInfo>,
}

impl AgentCatalog {
    /// Empty catalog — useful in unit tests + as a default before async load
    /// completes.
    pub fn new() -> Self {
        Self { agents: Vec::new() }
    }

    /// Build a catalog directly from a list of `AgentInfo`s. Stable insertion
    /// order is preserved.
    pub fn from_agents(agents: Vec<AgentInfo>) -> Self {
        Self { agents }
    }

    /// Parse a JSON document of the shape `[AgentInfo, …]`. This is the
    /// format returned by `GET /api/runtime/agents`.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        let agents: Vec<AgentInfo> = serde_json::from_str(json)?;
        Ok(Self { agents })
    }

    /// Walk a directory looking for `runtara_agent_*.meta.json` files. Pairs
    /// of `.wasm` + `.meta.json` are staged here by
    /// `scripts/build-agent-components.sh`. Missing or malformed meta.json is
    /// surfaced as an error so the caller can fail fast at boot.
    ///
    /// This is the server-side loader; the WASM validator uses
    /// [`from_json`](Self::from_json) instead.
    #[cfg(feature = "fs")]
    pub fn from_meta_dir(dir: &std::path::Path) -> std::io::Result<Self> {
        let mut agents: Vec<AgentInfo> = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !name.starts_with("runtara_agent_") || !name.ends_with(".meta.json") {
                continue;
            }
            let bytes = std::fs::read(&path)?;
            let info: AgentInfo = serde_json::from_slice(&bytes).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid {}: {e}", path.display()),
                )
            })?;
            agents.push(info);
        }
        // Deterministic order — keeps APIs returning the catalog stable.
        agents.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(Self { agents })
    }

    /// Borrow every agent. Order matches the underlying storage.
    pub fn agents(&self) -> &[AgentInfo] {
        &self.agents
    }

    /// True if any agent has the given id (matched canonically — `-`/`_` and
    /// ASCII case are equivalent; see [`canonical_agent_id`]).
    pub fn has_agent(&self, agent_id: &str) -> bool {
        let query = canonical_agent_id(agent_id);
        self.agents
            .iter()
            .any(|a| canonical_agent_id(&a.id) == query)
    }

    /// Look up an agent by id.
    ///
    /// Ids are matched canonically: the kebab `object-model` the catalog is
    /// keyed on and the snake `object_model` a legacy workflow might author
    /// both resolve to the same agent (see [`canonical_agent_id`]).
    pub fn agent(&self, agent_id: &str) -> Option<&AgentInfo> {
        let query = canonical_agent_id(agent_id);
        self.agents
            .iter()
            .find(|a| canonical_agent_id(&a.id) == query)
    }

    /// Look up a capability by `(agent_id, capability_id)`.
    pub fn capability(&self, agent_id: &str, capability_id: &str) -> Option<&CapabilityInfo> {
        self.agent(agent_id)?
            .capabilities
            .iter()
            .find(|c| c.id == capability_id)
    }

    /// Return the `integration_ids` of the agent matching `agent_id`
    /// (matched canonically; see [`canonical_agent_id`]), or an empty `Vec`
    /// if the agent isn't loaded.
    ///
    /// Used by the connections layer to translate user-facing
    /// "show me connections for agent <X>" queries into the
    /// `integration_id` list that the connection service stores on
    /// each row — so the connection service itself doesn't need to
    /// know what an "agent" is.
    pub fn integration_ids_for(&self, agent_id: &str) -> Vec<String> {
        let query = canonical_agent_id(agent_id);
        self.agents()
            .iter()
            .find(|a| canonical_agent_id(&a.id) == query)
            .map(|a| a.integration_ids.clone())
            .unwrap_or_default()
    }

    /// Number of agents in the catalog.
    pub fn len(&self) -> usize {
        self.agents.len()
    }

    /// True if the catalog has no agents (e.g. before discovery has run).
    pub fn is_empty(&self) -> bool {
        self.agents.is_empty()
    }
}

#[cfg(test)]
mod output_schema_tests {
    use super::*;

    const ADDRESS: OutputTypeMeta = OutputTypeMeta {
        type_name: "Address",
        display_name: Some("Address"),
        description: None,
        fields: &[OutputFieldMeta {
            name: "city",
            type_name: "String",
            display_name: None,
            description: None,
            example: None,
            nullable: false,
            items_type_name: None,
            nested_type_name: None,
        }],
    };

    const ORDER: OutputTypeMeta = OutputTypeMeta {
        type_name: "Order",
        display_name: None,
        description: None,
        fields: &[
            OutputFieldMeta {
                name: "total",
                type_name: "f64",
                display_name: None,
                description: None,
                example: None,
                nullable: false,
                items_type_name: None,
                nested_type_name: None,
            },
            OutputFieldMeta {
                name: "shipping_address",
                type_name: "Address",
                display_name: None,
                description: None,
                example: None,
                nullable: false,
                items_type_name: None,
                nested_type_name: Some("Address"),
            },
        ],
    };

    const CUSTOMER: OutputTypeMeta = OutputTypeMeta {
        type_name: "Customer",
        display_name: None,
        description: None,
        fields: &[
            OutputFieldMeta {
                name: "status_code",
                type_name: "u16",
                display_name: None,
                description: None,
                example: None,
                nullable: false,
                items_type_name: None,
                nested_type_name: None,
            },
            OutputFieldMeta {
                name: "retries",
                type_name: "Option<i64>",
                display_name: None,
                description: None,
                example: None,
                nullable: true,
                items_type_name: None,
                nested_type_name: None,
            },
            OutputFieldMeta {
                name: "address",
                type_name: "Address",
                display_name: None,
                description: None,
                example: None,
                nullable: false,
                items_type_name: None,
                nested_type_name: Some("Address"),
            },
            OutputFieldMeta {
                name: "orders",
                type_name: "Vec<Order>",
                display_name: None,
                description: None,
                example: None,
                nullable: false,
                items_type_name: Some("Order"),
                nested_type_name: None,
            },
            OutputFieldMeta {
                name: "tags",
                type_name: "Vec<String>",
                display_name: None,
                description: None,
                example: None,
                nullable: false,
                items_type_name: Some("String"),
                nested_type_name: None,
            },
        ],
    };

    /// Self-referential type: recursion must terminate via the visited set.
    const TREE_NODE: OutputTypeMeta = OutputTypeMeta {
        type_name: "TreeNode",
        display_name: None,
        description: None,
        fields: &[OutputFieldMeta {
            name: "child",
            type_name: "TreeNode",
            display_name: None,
            description: None,
            example: None,
            nullable: true,
            items_type_name: None,
            nested_type_name: Some("TreeNode"),
        }],
    };

    fn registry() -> OutputTypeRegistry<'static> {
        [
            ("Address", &ADDRESS),
            ("Order", &ORDER),
            ("Customer", &CUSTOMER),
            ("TreeNode", &TREE_NODE),
        ]
        .into_iter()
        .collect()
    }

    fn field(meta: &OutputTypeMeta, name: &str) -> OutputField {
        let registry = registry();
        meta.fields
            .iter()
            .find(|f| f.name == name)
            .map(|f| output_field_to_api_with_types(f, &registry))
            .expect("field exists")
    }

    #[test]
    fn integer_widths_map_to_integer() {
        assert_eq!(field(&CUSTOMER, "status_code").type_name, "integer");
    }

    #[test]
    fn option_types_map_to_inner_type() {
        let retries = field(&CUSTOMER, "retries");
        assert_eq!(retries.type_name, "integer");
        assert!(retries.nullable);
    }

    #[test]
    fn nested_struct_fields_resolve_inline() {
        let address = field(&CUSTOMER, "address");
        assert_eq!(address.type_name, "object");
        let fields = address.fields.expect("nested fields resolved");
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name, "city");
        assert_eq!(fields[0].type_name, "string");
    }

    #[test]
    fn struct_array_items_resolve_inline_and_recurse() {
        let orders = field(&CUSTOMER, "orders");
        assert_eq!(orders.type_name, "array");
        let items = orders.items.expect("item type resolved");
        assert_eq!(items.type_name, "object");
        let item_fields = items.fields.expect("item fields resolved");
        assert_eq!(item_fields[0].name, "total");
        assert_eq!(item_fields[0].type_name, "number");
        // Order.shipping_address recurses one more level into Address.
        assert_eq!(item_fields[1].type_name, "object");
        assert!(item_fields[1].fields.is_some());
    }

    #[test]
    fn primitive_array_items_carry_the_item_type() {
        let tags = field(&CUSTOMER, "tags");
        assert_eq!(tags.type_name, "array");
        assert_eq!(tags.items.expect("items").type_name, "string");
    }

    #[test]
    fn self_referential_types_terminate() {
        let child = field(&TREE_NODE, "child");
        assert_eq!(child.type_name, "object");
        // The visited set stops the recursion at the self-reference: the
        // outer field resolves, the inner one stays shapeless.
        let fields = child.fields.expect("first level resolved");
        assert_eq!(fields[0].name, "child");
        assert!(fields[0].fields.is_none());
    }

    #[test]
    fn without_registry_custom_types_are_unknown_not_string() {
        let f = CUSTOMER
            .fields
            .iter()
            .find(|f| f.name == "address")
            .unwrap();
        let converted = output_field_to_api(f);
        // Unregistered custom type: no fields, and no confident type claim —
        // the name may be a struct or a data enum (e.g. http's
        // HttpResponseBody), and the old catch-all "string" fed false
        // type-mismatch warnings in the editor.
        assert!(converted.fields.is_none());
        assert_eq!(converted.type_name, "any");
    }

    #[test]
    fn vec_of_unregistered_custom_type_has_unknown_items() {
        let f = OutputFieldMeta {
            name: "errors",
            type_name: "Vec<AgentBulkRowError>",
            display_name: None,
            description: None,
            example: None,
            nullable: false,
            items_type_name: Some("AgentBulkRowError"),
            nested_type_name: None,
        };
        let converted = output_field_to_api(&f);
        assert_eq!(converted.type_name, "array");
        // Elements are objects at runtime; claiming "string" (the old
        // fallback) misled MCP/API consumers. Unknown is honest.
        assert_eq!(converted.items.expect("items").type_name, "any");
    }

    #[test]
    fn nested_vec_items_stay_arrays() {
        // Vec<Vec<Value>>: trim_start_matches stripped repeated "Vec<"
        // prefixes and produced items "any" instead of "array".
        let f = OutputFieldMeta {
            name: "rows",
            type_name: "Vec<Vec<Value>>",
            display_name: None,
            description: None,
            example: None,
            nullable: false,
            items_type_name: Some("Vec<Value>"),
            nested_type_name: None,
        };
        let converted = output_field_to_api(&f);
        assert_eq!(converted.type_name, "array");
        assert_eq!(converted.items.expect("items").type_name, "array");
    }

    fn capability(output_type: &'static str) -> CapabilityMeta {
        CapabilityMeta {
            module: Some("test"),
            capability_id: "cap",
            function_name: "cap",
            input_type: "CapInput",
            output_type,
            display_name: None,
            description: None,
            has_side_effects: false,
            is_idempotent: true,
            rate_limited: false,
            compensation_hint: None,
            known_errors: &[],
            tags: &[],
        }
    }

    #[test]
    fn registered_struct_output_is_an_object() {
        let cap = capability("Customer");
        let info = capability_to_api_with_types(&cap, None, Some(&CUSTOMER), &registry());
        assert_eq!(info.output.type_name, "object");
        let fields = info.output.fields.expect("fields resolved");
        // Nested fields come through the registry now.
        let address = fields.iter().find(|f| f.name == "address").unwrap();
        assert!(address.fields.is_some());
    }

    #[test]
    fn vec_of_registered_struct_output_resolves_items() {
        let cap = capability("Vec<Order>");
        let info = capability_to_api_with_types(&cap, None, None, &registry());
        assert_eq!(info.output.type_name, "array");
        let items = info.output.items.expect("items resolved");
        assert_eq!(items.type_name, "object");
        assert!(items.fields.is_some());
    }

    #[test]
    fn unregistered_output_type_is_unknown() {
        let cap = capability("MysteryType");
        let info = capability_to_api_with_types(&cap, None, None, &registry());
        // Not registered: may be a struct or an enum — no confident claim.
        assert_eq!(info.output.type_name, "any");
    }

    #[test]
    fn plain_string_output_type_stays_string() {
        let cap = capability("String");
        let info = capability_to_api_with_types(&cap, None, None, &registry());
        assert_eq!(info.output.type_name, "string");
    }
}

#[cfg(test)]
mod catalog_tests {
    use super::*;

    fn sample_agent(id: &str) -> AgentInfo {
        AgentInfo {
            id: id.into(),
            name: id.into(),
            description: "test".into(),
            has_side_effects: false,
            supports_connections: false,
            integration_ids: vec![],
            capabilities: vec![CapabilityInfo {
                id: "hash".into(),
                name: "hash".into(),
                display_name: None,
                description: None,
                input_type: "HashInput".into(),
                inputs: vec![],
                output: FieldTypeInfo {
                    type_name: "HashResult".into(),
                    format: None,
                    display_name: None,
                    description: None,
                    items: None,
                    fields: None,
                    nullable: false,
                },
                has_side_effects: false,
                is_idempotent: true,
                rate_limited: false,
                compensation_hint: None,
                known_errors: vec![],
                tags: vec![],
            }],
        }
    }

    #[test]
    fn empty_catalog_has_no_agents() {
        let cat = AgentCatalog::new();
        assert_eq!(cat.len(), 0);
        assert!(cat.is_empty());
        assert!(cat.agent("missing").is_none());
    }

    #[test]
    fn from_agents_preserves_order() {
        let cat = AgentCatalog::from_agents(vec![sample_agent("b"), sample_agent("a")]);
        assert_eq!(
            cat.agents()
                .iter()
                .map(|a| a.id.as_str())
                .collect::<Vec<_>>(),
            vec!["b", "a"]
        );
    }

    #[test]
    fn lookup_by_id_and_capability() {
        let cat = AgentCatalog::from_agents(vec![sample_agent("crypto")]);
        assert!(cat.has_agent("crypto"));
        assert!(!cat.has_agent("missing"));
        assert!(cat.agent("crypto").is_some());
        assert!(cat.capability("crypto", "hash").is_some());
        assert!(cat.capability("crypto", "missing").is_none());
        assert!(cat.capability("missing", "hash").is_none());
    }

    #[test]
    fn lookup_normalizes_kebab_and_snake() {
        // The catalog is keyed on the canonical kebab id (as the component
        // dispatcher forces it), but a workflow may author either form. Both
        // must resolve to the same agent + capability.
        let cat = AgentCatalog::from_agents(vec![sample_agent("object-model")]);

        assert!(cat.has_agent("object-model"));
        assert!(cat.has_agent("object_model"));
        assert!(cat.has_agent("Object_Model"));
        assert_eq!(
            cat.agent("object_model").map(|a| a.id.as_str()),
            Some("object-model")
        );
        assert!(cat.capability("object_model", "hash").is_some());
        assert!(cat.capability("object-model", "hash").is_some());
        // Capability ids stay exact — they are always kebab.
        assert!(cat.capability("object_model", "missing").is_none());
    }

    #[test]
    fn integration_ids_for_returns_agent_integrations() {
        let mut agent = sample_agent("slack");
        agent.integration_ids = vec!["slack_oauth".into(), "slack_legacy".into()];
        let cat = AgentCatalog::from_agents(vec![agent]);

        assert_eq!(
            cat.integration_ids_for("slack"),
            vec!["slack_oauth".to_string(), "slack_legacy".to_string()],
        );
        assert_eq!(
            cat.integration_ids_for("SLACK"),
            cat.integration_ids_for("slack")
        );
        assert!(cat.integration_ids_for("missing").is_empty());
    }

    #[test]
    fn round_trip_json() {
        let original = AgentCatalog::from_agents(vec![sample_agent("crypto")]);
        let json = serde_json::to_string(&original.agents).unwrap();
        let parsed = AgentCatalog::from_json(&json).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed.agent("crypto").unwrap().id, "crypto");
    }

    #[test]
    fn from_json_rejects_garbage() {
        assert!(AgentCatalog::from_json("not-json").is_err());
    }

    #[cfg(feature = "fs")]
    #[test]
    fn from_meta_dir_loads_and_sorts() {
        use std::fs;
        let dir = tempfile::TempDir::new().expect("tempdir");
        let write = |id: &str| {
            let path = dir.path().join(format!("runtara_agent_{id}.meta.json"));
            fs::write(&path, serde_json::to_string(&sample_agent(id)).unwrap()).unwrap();
        };
        write("zeta");
        write("alpha");
        // Distractor files we should ignore.
        fs::write(dir.path().join("not-an-agent.json"), b"{}").unwrap();
        fs::write(
            dir.path().join("runtara_agent_zeta.wasm"),
            b"\0asm\x01\0\0\0",
        )
        .unwrap();

        let cat = AgentCatalog::from_meta_dir(dir.path()).expect("load");
        let ids: Vec<&str> = cat.agents().iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, vec!["alpha", "zeta"]);
    }

    #[cfg(feature = "fs")]
    #[test]
    fn from_meta_dir_surfaces_parse_errors() {
        use std::fs;
        let dir = tempfile::TempDir::new().expect("tempdir");
        fs::write(
            dir.path().join("runtara_agent_broken.meta.json"),
            b"{not json}",
        )
        .unwrap();
        let err = AgentCatalog::from_meta_dir(dir.path()).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("runtara_agent_broken.meta.json"));
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod workflow_agent_info_tests {
    use super::*;
    use crate::{SchemaField, SchemaFieldType};
    use std::collections::HashMap;

    fn field(field_type: SchemaFieldType, required: bool) -> SchemaField {
        SchemaField {
            field_type,
            description: None,
            required,
            default: None,
            example: None,
            items: None,
            enum_values: None,
            integration: None,
            label: None,
            placeholder: None,
            order: None,
            format: None,
            min: None,
            max: None,
            pattern: None,
            properties: None,
            visible_when: None,
            nullable: None,
        }
    }

    #[test]
    fn synthesizes_agent_info_from_workflow_schemas() {
        let mut input_schema = HashMap::new();
        input_schema.insert("message".to_string(), field(SchemaFieldType::String, true));
        input_schema.insert("count".to_string(), field(SchemaFieldType::Integer, false));
        let mut crm = field(SchemaFieldType::Connection, true);
        crm.integration = Some("hubspot".to_string());
        input_schema.insert("crm".to_string(), crm);
        let mut output_schema = HashMap::new();
        output_schema.insert("result".to_string(), field(SchemaFieldType::Object, true));

        let info = workflow_agent_info(
            "order-sync",
            "Order Sync",
            "Syncs orders",
            &input_schema,
            &output_schema,
        );

        assert_eq!(info.id, "order-sync");
        // A workflow-agent's connections are INPUTS, never a step-level
        // connection — supportsConnections=true would wrongly force parents to
        // configure a connection on the step. integrationIds stays
        // informational.
        assert!(!info.supports_connections);
        assert_eq!(info.integration_ids, vec!["hubspot"]);

        assert_eq!(info.capabilities.len(), 1);
        let cap = &info.capabilities[0];
        assert_eq!(cap.id, WORKFLOW_AGENT_CAPABILITY_ID);
        // Inputs are sorted for a deterministic (checksummable) meta.json.
        let names: Vec<&str> = cap.inputs.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["count", "crm", "message"]);
        // The connection input surfaces AS a typed input — no separate
        // connection-slots descriptor exists.
        let crm = cap.inputs.iter().find(|f| f.name == "crm").unwrap();
        assert_eq!(crm.type_name, "connection");
        assert!(crm.required);
        let message = cap.inputs.iter().find(|f| f.name == "message").unwrap();
        assert_eq!(message.type_name, "string");
        assert!(
            !cap.inputs
                .iter()
                .find(|f| f.name == "count")
                .unwrap()
                .required
        );

        let output_fields = cap.output.fields.as_ref().expect("output fields");
        assert_eq!(output_fields.len(), 1);
        assert_eq!(output_fields[0].name, "result");
        assert_eq!(output_fields[0].type_name, "object");

        // Conservative execution semantics for published workflows.
        assert!(cap.has_side_effects);
        assert!(!cap.is_idempotent);
        assert_eq!(cap.tags, vec!["workflow-agent"]);
    }

    #[test]
    fn synthesized_info_round_trips_like_a_meta_sidecar() {
        // The staged runtara_agent_<slug>.meta.json must parse back through the
        // exact loader path the dispatcher/catalog uses (serde on AgentInfo).
        let info = workflow_agent_info("echo", "Echo", "", &HashMap::new(), &HashMap::new());
        let json = serde_json::to_string_pretty(&info).expect("serializes");
        let parsed: AgentInfo = serde_json::from_str(&json).expect("parses back");
        assert_eq!(parsed.id, "echo");
        assert!(!parsed.supports_connections);
        assert!(parsed.integration_ids.is_empty());
        assert_eq!(parsed.capabilities[0].id, "run");
        // An empty output schema serializes without a fields key.
        assert!(parsed.capabilities[0].output.fields.is_none());
        // And the catalog folds it like any agent id.
        let catalog = AgentCatalog::from_agents(vec![parsed]);
        assert!(catalog.has_agent("echo"));
    }
}

#[cfg(test)]
mod slug_tests {
    use super::*;

    const WF_ID: &str = "a1b2c3d4-e5f6-7890-abcd-ef0123456789";

    #[test]
    fn generate_slugifies_names() {
        assert_eq!(generate_workflow_slug("Order Sync", WF_ID), "order-sync");
        assert_eq!(
            generate_workflow_slug("  HubSpot -> S3 (nightly!) ", WF_ID),
            "hubspot-s3-nightly"
        );
        // Leading digits survive — wit-parser accepts digit-led words
        // (asserted against the real parser in runtara-workflows).
        assert_eq!(generate_workflow_slug("2FA Sync", WF_ID), "2fa-sync");
        assert_eq!(generate_workflow_slug("My 2nd Flow", WF_ID), "my-2nd-flow");
        // Non-ASCII collapses into hyphens, never leaks through.
        assert_eq!(generate_workflow_slug("café Ünïcode", WF_ID), "caf-n-code");
    }

    #[test]
    fn generate_caps_length_and_retrims() {
        let long = "a".repeat(60) + " tail-of-name-that-overflows";
        let slug = generate_workflow_slug(&long, WF_ID);
        assert!(slug.len() <= WORKFLOW_SLUG_MAX_LEN, "{slug}");
        assert!(!slug.ends_with('-'), "truncation must re-trim: {slug}");
        // A hyphen landing exactly on the cap boundary is trimmed.
        let boundary = "a".repeat(WORKFLOW_SLUG_MAX_LEN - 1) + " b";
        let slug = generate_workflow_slug(&boundary, WF_ID);
        assert_eq!(slug, "a".repeat(WORKFLOW_SLUG_MAX_LEN - 1));
    }

    #[test]
    fn generate_falls_back_for_unnameable() {
        assert_eq!(generate_workflow_slug("", WF_ID), "wf-a1b2c3d4");
        assert_eq!(generate_workflow_slug("!!! ***", WF_ID), "wf-a1b2c3d4");
        // Even a hex-free workflow id yields something non-empty.
        assert_eq!(generate_workflow_slug("", "zzzz"), "wf");
    }

    #[test]
    fn generated_slugs_validate_and_are_canonical_fixpoints() {
        for name in [
            "Order Sync",
            "2FA sync",
            "",
            "!!!",
            "x",
            "A--B__C  D",
            "Ünïcode Überflow",
            &("very long ".repeat(30)),
        ] {
            let slug = generate_workflow_slug(name, WF_ID);
            validate_workflow_slug(&slug)
                .unwrap_or_else(|e| panic!("generated slug {slug:?} from {name:?} invalid: {e}"));
            assert_eq!(
                canonical_agent_id(&slug),
                slug,
                "slug must be a canonical_agent_id fixpoint"
            );
        }
    }

    #[test]
    fn validate_rejects_bad_shapes() {
        assert_eq!(validate_workflow_slug(""), Err(SlugError::Empty));
        assert_eq!(
            validate_workflow_slug("has_underscore"),
            Err(SlugError::InvalidChar('_'))
        );
        assert_eq!(
            validate_workflow_slug("Upper-Case"),
            Err(SlugError::InvalidChar('U'))
        );
        assert_eq!(
            validate_workflow_slug("-edge"),
            Err(SlugError::EdgeOrDoubleHyphen)
        );
        assert_eq!(
            validate_workflow_slug("edge-"),
            Err(SlugError::EdgeOrDoubleHyphen)
        );
        assert_eq!(
            validate_workflow_slug("dou--ble"),
            Err(SlugError::EdgeOrDoubleHyphen)
        );
        let too_long = "a".repeat(WORKFLOW_SLUG_MAX_LEN + 1);
        assert!(matches!(
            validate_workflow_slug(&too_long),
            Err(SlugError::TooLong { .. })
        ));
        // Leading digits are explicitly allowed.
        assert_eq!(validate_workflow_slug("2fa-sync"), Ok(()));
        assert_eq!(validate_workflow_slug("order-sync"), Ok(()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_agent_modules_count() {
        // Verify we have the expected number of built-in modules
        assert_eq!(
            BUILTIN_AGENT_MODULES.len(),
            11,
            "Expected 11 built-in agent modules"
        );
    }

    #[test]
    fn test_builtin_agent_modules_ids() {
        let ids: Vec<&str> = BUILTIN_AGENT_MODULES.iter().map(|m| m.id).collect();

        assert!(ids.contains(&"utils"), "Missing utils module");
        assert!(ids.contains(&"transform"), "Missing transform module");
        assert!(ids.contains(&"csv"), "Missing csv module");
        assert!(ids.contains(&"text"), "Missing text module");
        assert!(ids.contains(&"xml"), "Missing xml module");
        assert!(ids.contains(&"datetime"), "Missing datetime module");
        assert!(ids.contains(&"http"), "Missing http module");
        assert!(ids.contains(&"compression"), "Missing compression module");
        assert!(ids.contains(&"sftp"), "Missing sftp module");
        assert!(ids.contains(&"object-model"), "Missing object-model module");
    }

    #[test]
    fn test_builtin_agent_module_ids_are_canonical() {
        // Registered dispatcher modules are kebab-canonical; the builtin
        // list must match so id comparisons never depend on spelling.
        for module in BUILTIN_AGENT_MODULES {
            assert_eq!(
                module.id,
                canonical_agent_id(module.id),
                "builtin module id `{}` is not canonical kebab",
                module.id
            );
        }
    }

    #[test]
    fn test_get_all_agent_modules_includes_builtins() {
        let modules = get_all_agent_modules();

        // Should include all built-in modules
        assert!(
            modules.len() >= BUILTIN_AGENT_MODULES.len(),
            "get_all_agent_modules should include at least all built-in modules"
        );

        // Verify built-in module IDs are present
        let module_ids: Vec<&str> = modules.iter().map(|m| m.id).collect();
        for builtin in BUILTIN_AGENT_MODULES {
            assert!(
                module_ids.contains(&builtin.id),
                "Built-in module {} should be in get_all_agent_modules()",
                builtin.id
            );
        }
    }

    #[test]
    fn test_get_all_agent_modules_deduplication() {
        let modules = get_all_agent_modules();

        // Check for duplicates
        let mut seen_ids = std::collections::HashSet::new();
        for module in &modules {
            assert!(
                seen_ids.insert(module.id),
                "Duplicate module id found: {}",
                module.id
            );
        }
    }

    #[test]
    fn test_find_agent_module_existing() {
        let http_module = find_agent_module("http");
        assert!(http_module.is_some(), "Should find http module");

        let module = http_module.unwrap();
        assert_eq!(module.id, "http");
        assert_eq!(module.name, "HTTP");
        assert!(module.has_side_effects);
        assert!(module.supports_connections);
        assert!(module.secure);
    }

    #[test]
    fn test_find_agent_module_non_existing() {
        let result = find_agent_module("non_existent_module");
        assert!(result.is_none(), "Should not find non-existent module");
    }

    #[test]
    fn test_secure_modules() {
        // Only http and sftp should be secure
        for module in BUILTIN_AGENT_MODULES {
            match module.id {
                "http" | "sftp" => {
                    assert!(module.secure, "{} module should be secure", module.id);
                }
                _ => {
                    assert!(!module.secure, "{} module should not be secure", module.id);
                }
            }
        }
    }

    #[test]
    fn test_find_agent_module_legacy_snake_case() {
        // Legacy DSL graphs and operator config may still spell the id
        // snake_case; the lookup folds to the canonical kebab module.
        let module = find_agent_module("object_model");
        assert!(module.is_some(), "object_model should fold to object-model");
        assert_eq!(module.unwrap().id, "object-model");
    }

    #[test]
    fn test_side_effects_modules() {
        // http, sftp, and object-model have side effects
        for module in BUILTIN_AGENT_MODULES {
            match module.id {
                "http" | "sftp" | "object-model" => {
                    assert!(
                        module.has_side_effects,
                        "{} module should have side effects",
                        module.id
                    );
                }
                _ => {
                    assert!(
                        !module.has_side_effects,
                        "{} module should not have side effects",
                        module.id
                    );
                }
            }
        }
    }

    #[test]
    fn test_connection_supporting_modules() {
        // http, sftp, and object-model support connections
        for module in BUILTIN_AGENT_MODULES {
            match module.id {
                "http" | "sftp" | "object-model" => {
                    assert!(
                        module.supports_connections,
                        "{} module should support connections",
                        module.id
                    );
                    assert!(
                        !module.integration_ids.is_empty(),
                        "{} module should have integration IDs",
                        module.id
                    );
                }
                _ => {
                    assert!(
                        !module.supports_connections,
                        "{} module should not support connections",
                        module.id
                    );
                }
            }
        }
    }

    #[test]
    fn test_http_integration_ids() {
        let http_module = find_agent_module("http").unwrap();
        let integration_ids = http_module.integration_ids;

        assert!(
            integration_ids.contains(&"http_bearer"),
            "http should support http_bearer"
        );
        assert!(
            integration_ids.contains(&"http_api_key"),
            "http should support http_api_key"
        );
    }

    #[test]
    fn test_sftp_integration_ids() {
        let sftp_module = find_agent_module("sftp").unwrap();
        let integration_ids = sftp_module.integration_ids;

        assert!(
            integration_ids.contains(&"sftp"),
            "sftp should support sftp integration"
        );
    }

    // ========================================================================
    // Error Introspection Tests
    // ========================================================================

    #[test]
    fn test_error_kind_as_str() {
        assert_eq!(ErrorKind::Transient.as_str(), "transient");
        assert_eq!(ErrorKind::Permanent.as_str(), "permanent");
    }

    #[test]
    fn test_error_kind_equality() {
        assert_eq!(ErrorKind::Transient, ErrorKind::Transient);
        assert_eq!(ErrorKind::Permanent, ErrorKind::Permanent);
        assert_ne!(ErrorKind::Transient, ErrorKind::Permanent);
    }

    #[test]
    fn test_known_error_creation() {
        let error = KnownError {
            code: "TEST_ERROR",
            description: "A test error for validation",
            kind: ErrorKind::Transient,
            attributes: &["field1", "field2"],
        };

        assert_eq!(error.code, "TEST_ERROR");
        assert_eq!(error.description, "A test error for validation");
        assert_eq!(error.kind, ErrorKind::Transient);
        assert_eq!(error.attributes.len(), 2);
        assert!(error.attributes.contains(&"field1"));
        assert!(error.attributes.contains(&"field2"));
    }

    #[test]
    fn test_known_error_empty_attributes() {
        let error = KnownError {
            code: "SIMPLE_ERROR",
            description: "An error without attributes",
            kind: ErrorKind::Permanent,
            attributes: &[],
        };

        assert_eq!(error.code, "SIMPLE_ERROR");
        assert_eq!(error.kind, ErrorKind::Permanent);
        assert!(error.attributes.is_empty());
    }

    #[test]
    fn test_known_error_info_from_known_error() {
        let known_error = KnownError {
            code: "HTTP_TIMEOUT",
            description: "Request timed out",
            kind: ErrorKind::Transient,
            attributes: &["url", "timeout_ms"],
        };

        let info = KnownErrorInfo::from(&known_error);

        assert_eq!(info.code, "HTTP_TIMEOUT");
        assert_eq!(info.description, "Request timed out");
        assert_eq!(info.kind, "transient");
        assert_eq!(info.attributes.len(), 2);
        assert!(info.attributes.contains(&"url".to_string()));
        assert!(info.attributes.contains(&"timeout_ms".to_string()));
    }

    #[test]
    fn test_known_error_info_permanent_kind() {
        let known_error = KnownError {
            code: "VALIDATION_ERROR",
            description: "Input validation failed",
            kind: ErrorKind::Permanent,
            attributes: &["field"],
        };

        let info = KnownErrorInfo::from(&known_error);
        assert_eq!(info.kind, "permanent");
    }

    #[test]
    fn test_known_error_info_serialization() {
        let info = KnownErrorInfo {
            code: "NETWORK_ERROR".to_string(),
            description: "Network request failed".to_string(),
            kind: "transient".to_string(),
            attributes: vec!["url".to_string(), "status_code".to_string()],
        };

        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json.get("code").unwrap(), "NETWORK_ERROR");
        assert_eq!(json.get("description").unwrap(), "Network request failed");
        assert_eq!(json.get("kind").unwrap(), "transient");

        let attrs = json.get("attributes").unwrap().as_array().unwrap();
        assert_eq!(attrs.len(), 2);
    }

    #[test]
    fn test_known_error_info_serialization_empty_attributes_skipped() {
        let info = KnownErrorInfo {
            code: "SIMPLE_ERROR".to_string(),
            description: "A simple error".to_string(),
            kind: "permanent".to_string(),
            attributes: vec![],
        };

        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json.get("code").unwrap(), "SIMPLE_ERROR");
        // Empty attributes should be skipped due to skip_serializing_if
        assert!(json.get("attributes").is_none());
    }

    #[test]
    fn test_known_error_info_deserialization() {
        let json = serde_json::json!({
            "code": "RATE_LIMITED",
            "description": "Too many requests",
            "kind": "transient",
            "attributes": ["retry_after_ms"]
        });

        let info: KnownErrorInfo = serde_json::from_value(json).unwrap();
        assert_eq!(info.code, "RATE_LIMITED");
        assert_eq!(info.description, "Too many requests");
        assert_eq!(info.kind, "transient");
        assert_eq!(info.attributes, vec!["retry_after_ms"]);
    }

    #[test]
    fn test_known_error_info_deserialization_without_attributes() {
        let json = serde_json::json!({
            "code": "AUTH_ERROR",
            "description": "Authentication failed",
            "kind": "permanent"
        });

        let info: KnownErrorInfo = serde_json::from_value(json).unwrap();
        assert_eq!(info.code, "AUTH_ERROR");
        assert_eq!(info.kind, "permanent");
        // Attributes should default to empty vec
        assert!(info.attributes.is_empty());
    }

    #[test]
    fn test_capability_info_with_known_errors() {
        let info = CapabilityInfo {
            id: "http-request".to_string(),
            name: "http_request".to_string(),
            display_name: Some("HTTP Request".to_string()),
            description: Some("Make HTTP request".to_string()),
            input_type: "HttpRequestInput".to_string(),
            inputs: vec![],
            output: FieldTypeInfo {
                type_name: "object".to_string(),
                format: None,
                display_name: None,
                description: None,
                fields: None,
                items: None,
                nullable: false,
            },
            has_side_effects: true,
            is_idempotent: false,
            rate_limited: false,
            compensation_hint: None,
            known_errors: vec![
                KnownErrorInfo {
                    code: "NETWORK_ERROR".to_string(),
                    description: "Network failed".to_string(),
                    kind: "transient".to_string(),
                    attributes: vec!["url".to_string()],
                },
                KnownErrorInfo {
                    code: "HTTP_4XX".to_string(),
                    description: "Client error".to_string(),
                    kind: "permanent".to_string(),
                    attributes: vec!["url".to_string(), "status_code".to_string()],
                },
            ],
            tags: vec![],
        };

        let json = serde_json::to_value(&info).unwrap();
        let errors = json.get("knownErrors").unwrap().as_array().unwrap();
        assert_eq!(errors.len(), 2);

        // First error
        assert_eq!(errors[0].get("code").unwrap(), "NETWORK_ERROR");
        assert_eq!(errors[0].get("kind").unwrap(), "transient");

        // Second error
        assert_eq!(errors[1].get("code").unwrap(), "HTTP_4XX");
        assert_eq!(errors[1].get("kind").unwrap(), "permanent");
    }

    #[test]
    fn test_capability_info_empty_known_errors_skipped() {
        let info = CapabilityInfo {
            id: "random-double".to_string(),
            name: "random_double".to_string(),
            display_name: None,
            description: None,
            input_type: "RandomDoubleInput".to_string(),
            inputs: vec![],
            output: FieldTypeInfo {
                type_name: "number".to_string(),
                format: Some("double".to_string()),
                display_name: None,
                description: None,
                fields: None,
                items: None,
                nullable: false,
            },
            has_side_effects: false,
            is_idempotent: true,
            rate_limited: false,
            compensation_hint: None,
            known_errors: vec![],
            tags: vec![],
        };

        let json = serde_json::to_value(&info).unwrap();
        // Empty knownErrors should be skipped due to skip_serializing_if
        assert!(json.get("knownErrors").is_none());
    }
}
