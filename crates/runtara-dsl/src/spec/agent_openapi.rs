// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Agent OpenAPI Specification Generator
//!
//! Generates OpenAPI 3.1 specification for agents that matches
//! the exact format returned by the API endpoints.

use serde_json::{Value, json};
use std::collections::HashMap;

/// Current agent registry version
pub const AGENT_VERSION: &str = "1.0.0";

/// Generate OpenAPI specification for agents
pub fn generate_agent_openapi_spec(agents: Vec<Value>) -> Value {
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Runtara Agents API",
            "version": AGENT_VERSION,
            "description": "Agent registry and capability specifications for Runtara"
        },
        "servers": [
            {
                "url": "http://localhost:7001/api/runtime",
                "description": "Local development server"
            }
        ],
        "paths": generate_paths(),
        "components": {
            "schemas": generate_schemas(&agents),
            "securitySchemes": {
                "TenantAuth": {
                    "type": "apiKey",
                    "in": "header",
                    "name": "X-Org-Id"
                }
            }
        },
        "security": [
            {"TenantAuth": []}
        ]
    })
}

/// Generate API path definitions
fn generate_paths() -> Value {
    json!({
        "/agents": {
            "get": {
                "summary": "List all agents",
                "operationId": "listAgents",
                "responses": {
                    "200": {
                        "description": "List of available agents",
                        "content": {
                            "application/json": {
                                "schema": {
                                    "$ref": "#/components/schemas/ListAgentsResponse"
                                }
                            }
                        }
                    }
                }
            }
        },
        "/agents/{agentId}": {
            "get": {
                "summary": "Get agent details",
                "operationId": "getAgent",
                "parameters": [
                    {
                        "name": "agentId",
                        "in": "path",
                        "required": true,
                        "schema": {"type": "string"}
                    }
                ],
                "responses": {
                    "200": {
                        "description": "Agent details with all capabilities",
                        "content": {
                            "application/json": {
                                "schema": {
                                    "$ref": "#/components/schemas/AgentInfo"
                                }
                            }
                        }
                    },
                    "404": {
                        "description": "Agent not found"
                    }
                }
            }
        },
        "/agents/{agentId}/capabilities/{capabilityId}": {
            "get": {
                "summary": "Get capability details",
                "operationId": "getCapability",
                "parameters": [
                    {
                        "name": "agentId",
                        "in": "path",
                        "required": true,
                        "schema": {"type": "string"}
                    },
                    {
                        "name": "capabilityId",
                        "in": "path",
                        "required": true,
                        "schema": {"type": "string"}
                    }
                ],
                "responses": {
                    "200": {
                        "description": "Capability details",
                        "content": {
                            "application/json": {
                                "schema": {
                                    "$ref": "#/components/schemas/CapabilityInfo"
                                }
                            }
                        }
                    },
                    "404": {
                        "description": "Capability not found"
                    }
                }
            }
        },
        "/agents/{agentId}/capabilities/{capabilityId}/test": {
            "post": {
                "summary": "Test an agent capability",
                "description": "Requires ENABLE_AGENT_TESTING=true",
                "operationId": "testCapability",
                "parameters": [
                    {
                        "name": "agentId",
                        "in": "path",
                        "required": true,
                        "schema": {"type": "string"}
                    },
                    {
                        "name": "capabilityId",
                        "in": "path",
                        "required": true,
                        "schema": {"type": "string"}
                    }
                ],
                "requestBody": {
                    "required": true,
                    "content": {
                        "application/json": {
                            "schema": {
                                "type": "object",
                                "required": ["input"],
                                "properties": {
                                    "input": {
                                        "type": "object",
                                        "description": "Capability input matching the capability's input schema"
                                    }
                                }
                            }
                        }
                    }
                },
                "responses": {
                    "200": {
                        "description": "Test execution result",
                        "content": {
                            "application/json": {
                                "schema": {
                                    "$ref": "#/components/schemas/TestCapabilityResponse"
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}

/// Generate schema definitions that match the API response format exactly
fn generate_schemas(agents: &[Value]) -> Value {
    let mut schemas: HashMap<String, Value> = HashMap::new();

    // ListAgentsResponse - summary list
    schemas.insert(
        "ListAgentsResponse".to_string(),
        json!({
            "type": "object",
            "properties": {
                "agents": {
                    "type": "array",
                    "items": {
                        "$ref": "#/components/schemas/AgentSummary"
                    }
                }
            }
        }),
    );

    // AgentSummary - used in list response
    schemas.insert(
        "AgentSummary".to_string(),
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string"},
                "name": {"type": "string"},
                "description": {"type": "string"}
            }
        }),
    );

    // AgentInfo - full agent details (matches API exactly)
    schemas.insert(
        "AgentInfo".to_string(),
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string"},
                "name": {"type": "string"},
                "description": {"type": "string"},
                "hasSideEffects": {"type": "boolean"},
                "supportsConnections": {"type": "boolean"},
                "integrationIds": {
                    "type": "array",
                    "items": {"type": "string"}
                },
                "capabilities": {
                    "type": "array",
                    "items": {
                        "$ref": "#/components/schemas/CapabilityInfo"
                    }
                },
                "interfaces": {
                    "type": "array",
                    "items": {
                        "$ref": "#/components/schemas/InterfaceSupport"
                    }
                }
            }
        }),
    );

    // CapabilityInfo - capability details (matches API exactly)
    schemas.insert(
        "CapabilityInfo".to_string(),
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Capability ID (kebab-case)"},
                "name": {"type": "string", "description": "Function name"},
                "displayName": {"type": ["string", "null"]},
                "description": {"type": ["string", "null"]},
                "inputType": {"type": "string", "description": "Rust input struct name"},
                "inputs": {
                    "type": "array",
                    "items": {
                        "$ref": "#/components/schemas/CapabilityField"
                    }
                },
                "output": {
                    "$ref": "#/components/schemas/FieldTypeInfo"
                },
                "hasSideEffects": {"type": "boolean"},
                "isIdempotent": {"type": "boolean"},
                "isInterfaceCapability": {"type": "boolean"},
                "interfaceCategory": {"type": ["string", "null"]}
            }
        }),
    );

    // CapabilityField - input field metadata
    schemas.insert(
        "CapabilityField".to_string(),
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "displayName": {"type": ["string", "null"]},
                "description": {"type": ["string", "null"]},
                "type": {
                    "type": "string",
                    "enum": ["string", "number", "integer", "boolean", "array", "object", "file"]
                },
                "format": {"type": ["string", "null"], "description": "e.g., double, int64, date-time, binary"},
                "items": {
                    "oneOf": [
                        {"type": "null"},
                        {"$ref": "#/components/schemas/FieldTypeInfo"}
                    ]
                },
                "required": {"type": "boolean"},
                "default": {"description": "Default value as JSON"},
                "example": {"description": "Example value"},
                "enum": {
                    "type": ["array", "null"],
                    "items": {"type": "string"}
                }
            }
        }),
    );

    // FieldTypeInfo - type information for outputs
    schemas.insert(
        "FieldTypeInfo".to_string(),
        json!({
            "type": "object",
            "properties": {
                "type": {
                    "type": "string",
                    "enum": ["string", "number", "integer", "boolean", "array", "object", "file", "null"]
                },
                "format": {"type": ["string", "null"]},
                "displayName": {"type": ["string", "null"]},
                "description": {"type": ["string", "null"]},
                "fields": {
                    "type": ["array", "null"],
                    "items": {
                        "$ref": "#/components/schemas/OutputField"
                    }
                }
            }
        }),
    );

    // OutputField - for structured outputs
    schemas.insert(
        "OutputField".to_string(),
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "type": {
                    "type": "string",
                    "enum": ["string", "number", "integer", "boolean", "array", "object", "file"]
                },
                "description": {"type": ["string", "null"]}
            }
        }),
    );

    // InterfaceSupport - interface information
    schemas.insert(
        "InterfaceSupport".to_string(),
        json!({
            "type": "object",
            "properties": {
                "interface": {"type": "string"},
                "capabilities": {
                    "type": "array",
                    "items": {"type": "string"}
                }
            }
        }),
    );

    // TestCapabilityResponse
    schemas.insert(
        "TestCapabilityResponse".to_string(),
        json!({
            "type": "object",
            "properties": {
                "success": {"type": "boolean"},
                "output": {"description": "Capability output"},
                "error": {"type": ["string", "null"]},
                "executionTimeMs": {"type": "number"}
            }
        }),
    );

    // FileData - base64-encoded file with optional metadata
    schemas.insert(
        "FileData".to_string(),
        json!({
            "type": "object",
            "description": "Base64-encoded file data with optional metadata. Can also be provided as a plain base64 string.",
            "required": ["content"],
            "properties": {
                "content": {
                    "type": "string",
                    "description": "Base64-encoded file content"
                },
                "filename": {
                    "type": "string",
                    "description": "Original filename (optional)"
                },
                "mimeType": {
                    "type": "string",
                    "description": "MIME type, e.g., 'text/csv', 'application/pdf' (optional)"
                }
            }
        }),
    );

    // Generate capability-specific input schemas for each agent
    for agent in agents {
        if let Some(capabilities) = agent.get("capabilities").and_then(|o| o.as_array()) {
            let agent_id = agent.get("id").and_then(|id| id.as_str()).unwrap_or("");

            for capability in capabilities {
                let capability_id = capability
                    .get("id")
                    .and_then(|id| id.as_str())
                    .unwrap_or("");
                let schema_name = format!(
                    "{}_{}_input",
                    agent_id.replace('-', "_"),
                    capability_id.replace('-', "_")
                );

                // Generate input schema from capability inputs
                if let Some(inputs) = capability.get("inputs").and_then(|i| i.as_array()) {
                    let mut properties = HashMap::new();
                    let mut required = Vec::new();

                    for input in inputs {
                        if let Some(name) = input.get("name").and_then(|n| n.as_str()) {
                            let field_schema = generate_field_schema(input);
                            properties.insert(name.to_string(), field_schema);

                            if input
                                .get("required")
                                .and_then(|r| r.as_bool())
                                .unwrap_or(false)
                            {
                                required.push(name.to_string());
                            }
                        }
                    }

                    schemas.insert(
                        schema_name,
                        json!({
                            "type": "object",
                            "properties": properties,
                            "required": required
                        }),
                    );
                }
            }
        }
    }

    json!(schemas)
}

/// Generate JSON Schema for a field based on its metadata
fn generate_field_schema(field: &Value) -> Value {
    let field_type = field.get("type").and_then(|t| t.as_str());

    // For file type, reference the FileData schema with oneOf to allow string or object
    if field_type == Some("file") {
        let mut schema = json!({
            "oneOf": [
                {"type": "string", "description": "Plain base64-encoded content"},
                {"$ref": "#/components/schemas/FileData"}
            ]
        });

        if let Some(description) = field.get("description").and_then(|d| d.as_str()) {
            schema["description"] = json!(description);
        }

        return schema;
    }

    let mut schema = json!({});

    if let Some(field_type) = field_type {
        schema["type"] = json!(field_type);
    }

    if let Some(format) = field.get("format").and_then(|f| f.as_str()) {
        schema["format"] = json!(format);
    }

    if let Some(description) = field.get("description").and_then(|d| d.as_str()) {
        schema["description"] = json!(description);
    }

    if let Some(enum_values) = field.get("enum").and_then(|e| e.as_array()) {
        schema["enum"] = json!(enum_values);
    }

    if let Some(default) = field.get("default") {
        schema["default"] = default.clone();
    }

    if let Some(example) = field.get("example") {
        schema["example"] = example.clone();
    }

    // Handle array items
    if field_type == Some("array")
        && let Some(items) = field.get("items")
    {
        schema["items"] = generate_field_schema(items);
    }

    schema
}

/// Get agent changelog for version tracking
pub fn get_agent_changelog() -> Value {
    json!({
        "version": AGENT_VERSION,
        "changes": [
            {
                "version": "1.0.0",
                "date": "2024-11-24",
                "breaking": false,
                "changes": [
                    {
                        "type": "added",
                        "agent": "transform",
                        "capability": "group-by",
                        "description": "Added group-by capability to replace GroupBy step type"
                    }
                ]
            }
        ]
    })
}
