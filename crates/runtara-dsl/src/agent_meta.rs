// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Agent capability metadata types for runtime introspection
//!
//! These types are used by the `runtara-agent-macro` crate to generate
//! metadata that can be collected at runtime using the `inventory` crate.

use std::future::Future;
use std::pin::Pin;

/// Trait for types that can provide their enum variant names.
/// Used by the CapabilityInput macro to extract enum values for API metadata.
pub trait EnumVariants {
    /// Returns the variant names as they appear in JSON serialization
    fn variant_names() -> &'static [&'static str];
}

/// Function pointer type for getting enum variant names
pub type EnumVariantsFn = fn() -> &'static [&'static str];

/// Async executor function type - returns a boxed future.
/// This allows both sync and async capabilities to be executed uniformly:
/// - Async capabilities return futures directly
/// - Sync capabilities are wrapped with tokio::task::spawn_blocking
pub type CapabilityExecutorFn =
    fn(
        serde_json::Value,
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, String>> + Send>>;

/// Executor for an agent capability - registered via inventory
pub struct CapabilityExecutor {
    /// The agent module name (e.g., "utils", "transform")
    pub module: &'static str,
    /// Capability ID in kebab-case (e.g., "random-double")
    pub capability_id: &'static str,
    /// The executor function
    pub execute: CapabilityExecutorFn,
}

// Register CapabilityExecutor with inventory
inventory::collect!(&'static CapabilityExecutor);

/// Execute a capability by module and capability_id using inventory-registered executors.
/// This is an async function that awaits the capability's future.
pub async fn execute_capability(
    module: &str,
    capability_id: &str,
    input: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let module_lower = module.to_lowercase();

    for executor in inventory::iter::<&'static CapabilityExecutor> {
        if executor.module == module_lower && executor.capability_id == capability_id {
            return (executor.execute)(input).await;
        }
    }

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
}

// Register CapabilityMeta with inventory
inventory::collect!(&'static CapabilityMeta);

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

// Register InputTypeMeta with inventory
inventory::collect!(&'static InputTypeMeta);

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

// Register OutputTypeMeta with inventory
inventory::collect!(&'static OutputTypeMeta);

/// Get all registered capability metadata
pub fn get_all_capabilities() -> impl Iterator<Item = &'static CapabilityMeta> {
    inventory::iter::<&'static CapabilityMeta>
        .into_iter()
        .copied()
}

/// Get all registered input type metadata
pub fn get_all_input_types() -> impl Iterator<Item = &'static InputTypeMeta> {
    inventory::iter::<&'static InputTypeMeta>
        .into_iter()
        .copied()
}

/// Get all registered output type metadata
pub fn get_all_output_types() -> impl Iterator<Item = &'static OutputTypeMeta> {
    inventory::iter::<&'static OutputTypeMeta>
        .into_iter()
        .copied()
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
    #[serde(rename = "knownErrors", skip_serializing_if = "Vec::is_empty")]
    pub known_errors: Vec<KnownErrorInfo>,
}

/// API-compatible capability field info.
/// Used for agent inputs and scenario input/output schemas.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
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
    #[serde(skip_serializing_if = "Option::is_none", skip_deserializing)]
    pub fields: Option<Box<Vec<OutputField>>>,
    /// For array types, describes the item type
    #[serde(skip_serializing_if = "Option::is_none", skip_deserializing)]
    pub items: Option<Box<FieldTypeInfo>>,
    /// Whether this field can be null
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub nullable: bool,
}

/// API-compatible output field info.
/// Describes an output field with type information.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
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
    #[serde(skip_serializing_if = "Option::is_none", skip_deserializing)]
    pub items: Option<Box<FieldTypeInfo>>,
    /// For nested object types, the fields of the nested object
    #[serde(skip_serializing_if = "Option::is_none", skip_deserializing)]
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

// Register AgentModuleConfig with inventory
inventory::collect!(&'static AgentModuleConfig);

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
        integration_ids: &["bearer", "api_key", "basic_auth"],
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
        id: "file",
        name: "File",
        description: "File system capabilities for reading, writing, listing, copying, moving, and deleting files within the workflow workspace",
        has_side_effects: true,
        supports_connections: false,
        integration_ids: &[],
        secure: false,
    },
    AgentModuleConfig {
        id: "object_model",
        name: "Object Model",
        description: "Object Model capabilities for database CRUD operations - create, query, and check instances in object model schemas (has side effects)",
        has_side_effects: true,
        supports_connections: false,
        integration_ids: &[],
        secure: false,
    },
];

/// Get all agent modules (built-in + inventory-registered).
/// Built-in modules take precedence over inventory-registered ones with the same id.
/// Modules are deduplicated by id.
pub fn get_all_agent_modules() -> Vec<&'static AgentModuleConfig> {
    use std::collections::HashSet;

    let mut seen_ids = HashSet::new();
    let mut modules = Vec::new();

    // Add built-in modules first (they take precedence)
    for module in BUILTIN_AGENT_MODULES {
        if seen_ids.insert(module.id) {
            modules.push(module);
        }
    }

    // Add inventory-registered modules (skip if id already exists)
    for module in inventory::iter::<&'static AgentModuleConfig> {
        if seen_ids.insert(module.id) {
            modules.push(*module);
        }
    }

    modules
}

/// Find agent module config by id
pub fn find_agent_module(id: &str) -> Option<&'static AgentModuleConfig> {
    get_all_agent_modules().into_iter().find(|m| m.id == id)
}

// ============================================================================
// Step Type Metadata (for automatic DSL generation)
// ============================================================================

/// Function pointer type for generating JSON schema
pub type SchemaGeneratorFn = fn() -> schemars::schema::RootSchema;

/// Metadata for a step type - registered via inventory
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

// Register StepTypeMeta with inventory
inventory::collect!(&'static StepTypeMeta);

/// Get all registered step type metadata
pub fn get_all_step_types() -> impl Iterator<Item = &'static StepTypeMeta> {
    inventory::iter::<&'static StepTypeMeta>
        .into_iter()
        .copied()
}

/// Find step type metadata by id
pub fn find_step_type(id: &str) -> Option<&'static StepTypeMeta> {
    get_all_step_types().find(|m| m.id == id)
}

// ============================================================================
// Connection Type Metadata (for connection form generation)
// ============================================================================

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
    /// Default value as JSON string
    pub default_value: Option<&'static str>,
    /// Whether this is a secret field (password, API key, etc.)
    pub is_secret: bool,
}

/// Metadata for a connection type - registered via inventory
#[derive(Debug, Clone)]
pub struct ConnectionTypeMeta {
    /// Unique identifier for this connection type (e.g., "bearer", "sftp")
    pub integration_id: &'static str,
    /// Display name for UI (e.g., "Bearer Token", "SFTP")
    pub display_name: &'static str,
    /// Description of this connection type
    pub description: Option<&'static str>,
    /// Category for grouping (e.g., "ecommerce", "file_storage", "llm")
    pub category: Option<&'static str>,
    /// Fields required for this connection type
    pub fields: &'static [ConnectionFieldMeta],
}

// Register ConnectionTypeMeta with inventory
inventory::collect!(&'static ConnectionTypeMeta);

/// Get all registered connection type metadata
pub fn get_all_connection_types() -> impl Iterator<Item = &'static ConnectionTypeMeta> {
    inventory::iter::<&'static ConnectionTypeMeta>
        .into_iter()
        .copied()
}

/// Find connection type metadata by integration_id
pub fn find_connection_type(integration_id: &str) -> Option<&'static ConnectionTypeMeta> {
    get_all_connection_types().find(|m| m.integration_id == integration_id)
}

// ============================================================================
// Conversion Functions (inventory metadata -> API types)
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
        "i32" | "i64" | "u32" | "u64" | "usize" => TypeConversionResult {
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
        t if t.starts_with("Vec<") => {
            let inner = t.trim_start_matches("Vec<").trim_end_matches('>');
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

/// Get JSON Schema for ConditionExpression type
fn get_condition_expression_schema() -> serde_json::Value {
    use serde_json::json;

    // Generate schema using schemars (types are in crate root via include!)
    let schema = schemars::schema_for!(crate::ConditionExpression);
    serde_json::to_value(schema).unwrap_or_else(|_| {
        // Fallback to manual schema if schemars fails
        json!({
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
    })
}

/// Convert InputFieldMeta to CapabilityField
fn input_field_to_api(field: &InputFieldMeta) -> CapabilityField {
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

/// Convert OutputFieldMeta to OutputField
fn output_field_to_api(field: &OutputFieldMeta) -> OutputField {
    let (type_name, format, _) = rust_to_json_schema_type(field.type_name);

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
        // Note: items and fields are populated by frontend lookup using items_type_name/nested_type_name
        // We don't recursively resolve here to avoid serde Deserialize stack overflow issues
        items: None,
        fields: None,
    }
}

/// Convert CapabilityMeta to CapabilityInfo
fn capability_to_api(
    cap: &CapabilityMeta,
    input_type_meta: Option<&InputTypeMeta>,
    output_type_meta: Option<&OutputTypeMeta>,
) -> CapabilityInfo {
    let (output_type, output_format, _) = rust_to_json_schema_type(cap.output_type);

    let inputs = input_type_meta
        .map(|m| m.fields.iter().map(input_field_to_api).collect())
        .unwrap_or_default();

    let output_fields = output_type_meta
        .map(|m| Box::new(m.fields.iter().map(output_field_to_api).collect::<Vec<_>>()));

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
            items: None,
            nullable: false,
        },
        has_side_effects: cap.has_side_effects,
        is_idempotent: cap.is_idempotent,
        rate_limited: cap.rate_limited,
        compensation_hint,
        known_errors,
    }
}

/// Build API-compatible agent list from inventory-registered metadata
pub fn get_agents() -> Vec<AgentInfo> {
    use std::collections::HashMap;

    // Collect all input types into a map for lookup
    let input_types: HashMap<&str, &InputTypeMeta> =
        get_all_input_types().map(|m| (m.type_name, m)).collect();

    // Collect all output types into a map for lookup
    let output_types: HashMap<&str, &OutputTypeMeta> =
        get_all_output_types().map(|m| (m.type_name, m)).collect();

    // Group capabilities by module
    let mut caps_by_module: HashMap<&str, Vec<&CapabilityMeta>> = HashMap::new();
    for cap in get_all_capabilities() {
        let module = cap.module.unwrap_or("unknown");
        caps_by_module.entry(module).or_default().push(cap);
    }

    // Build agent info for each module
    let mut agents = Vec::new();

    for config in get_all_agent_modules() {
        let caps = caps_by_module.get(config.id).cloned().unwrap_or_default();

        if caps.is_empty() {
            continue;
        }

        let capabilities: Vec<CapabilityInfo> = caps
            .iter()
            .map(|cap| {
                let input_meta = input_types.get(cap.input_type).copied();
                let output_meta = output_types.get(cap.output_type).copied();
                capability_to_api(cap, input_meta, output_meta)
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
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_agent_modules_count() {
        // Verify we have the expected number of built-in modules
        assert_eq!(
            BUILTIN_AGENT_MODULES.len(),
            12,
            "Expected 12 built-in agent modules"
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
        assert!(ids.contains(&"file"), "Missing file module");
        assert!(ids.contains(&"sftp"), "Missing sftp module");
        assert!(ids.contains(&"object_model"), "Missing object_model module");
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
    fn test_side_effects_modules() {
        // http, sftp, file, and object_model have side effects
        for module in BUILTIN_AGENT_MODULES {
            match module.id {
                "http" | "sftp" | "file" | "object_model" => {
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
        // Only http and sftp support connections
        for module in BUILTIN_AGENT_MODULES {
            match module.id {
                "http" | "sftp" => {
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
            integration_ids.contains(&"bearer"),
            "http should support bearer"
        );
        assert!(
            integration_ids.contains(&"api_key"),
            "http should support api_key"
        );
        assert!(
            integration_ids.contains(&"basic_auth"),
            "http should support basic_auth"
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
        };

        let json = serde_json::to_value(&info).unwrap();
        // Empty knownErrors should be skipped due to skip_serializing_if
        assert!(json.get("knownErrors").is_none());
    }
}
