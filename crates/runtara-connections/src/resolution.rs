//! Safe, tenant-scoped connection metadata and resource-discovery contracts.
//!
//! A workflow carries only an opaque connection id. The host resolves that id
//! to [`ConnectionDescriptor`] before invoking a connection-aware component.
//! Each connection-specific extractor advertises the resources it can resolve;
//! credentials never appear in the descriptor, request, or normalized page.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::ConnectionStatus;

/// A non-secret description of one tenant-owned connection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ConnectionDescriptor {
    pub connection_id: String,
    pub integration_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_subtype: Option<String>,
    pub status: ConnectionStatus,
    /// Connection-local resource names advertised by the owning extractor.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<ConnectionResourceDefinition>,
    /// Explicitly safe, integration-owned metadata. It must never contain
    /// connection parameters, credentials, tokens, or resolved auth headers.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
}

impl ConnectionDescriptor {
    pub fn resource(&self, name: &str) -> Option<&ConnectionResourceDefinition> {
        self.resources.iter().find(|resource| resource.name == name)
    }
}

/// One resource catalog exposed by a connection-specific extractor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ConnectionResourceDefinition {
    /// Connection-local name, for example `models` or `sqs.queues`.
    pub name: String,
    pub description: String,
}

/// Generic request for connection-backed resources.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConnectionResourceRequest {
    pub resource_name: String,
    /// Optional free-text narrowing. The connection extractor translates it
    /// to the provider's native search/prefix behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

impl ConnectionResourceRequest {
    pub fn search(&self) -> Option<&str> {
        self.search
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    }
}

/// One normalized option returned by a connection-backed extractor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ConnectionResourceItem {
    pub value: Value,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
}

/// A normalized, optionally paginated resource-discovery result.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ConnectionResourcePage {
    pub items: Vec<ConnectionResourceItem>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub fetched_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub stale: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_accepts_only_generic_search_and_pagination() {
        let request: ConnectionResourceRequest = serde_json::from_value(serde_json::json!({
            "resourceName": "sqs.queues",
            "search": " orders ",
            "cursor": "next",
            "limit": 25
        }))
        .unwrap();

        assert_eq!(request.resource_name, "sqs.queues");
        assert_eq!(request.search(), Some("orders"));
        assert_eq!(request.cursor.as_deref(), Some("next"));
        assert_eq!(request.limit, Some(25));
    }

    #[test]
    fn provider_arguments_are_not_part_of_the_contract() {
        let error = serde_json::from_value::<ConnectionResourceRequest>(serde_json::json!({
            "resourceName": "bedrock.models",
            "arguments": {"provider": "Anthropic"}
        }))
        .unwrap_err();

        assert!(error.to_string().contains("unknown field `arguments`"));
    }

    #[test]
    fn descriptor_resource_lookup_is_connection_local() {
        let descriptor = ConnectionDescriptor {
            connection_id: "conn".into(),
            integration_id: "openai_api_key".into(),
            connection_subtype: None,
            status: ConnectionStatus::Active,
            resources: vec![ConnectionResourceDefinition {
                name: "models".into(),
                description: "Available OpenAI models".into(),
            }],
            metadata: Value::Null,
        };

        assert!(descriptor.resource("models").is_some());
        assert!(descriptor.resource("sqs.queues").is_none());
    }
}
