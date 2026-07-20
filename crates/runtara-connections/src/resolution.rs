//! Safe, tenant-scoped connection metadata and resource-discovery contracts.
//!
//! A workflow carries only an opaque connection id. The host resolves that id
//! to [`ConnectionDescriptor`] before invoking a connection-aware component.
//! Provider-specific discovery (models, queues, buckets, …) uses the same
//! descriptor plus the generic request/page types below; credentials never
//! appear in any of these values.

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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub features: Vec<ConnectionFeature>,
    /// Explicitly safe, integration-owned metadata. It must never contain
    /// connection parameters, credentials, tokens, or resolved auth headers.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
}

impl ConnectionDescriptor {
    /// Find a semantic feature such as `ai.chat` or `llm.models`.
    pub fn feature(&self, key: &str) -> Option<&ConnectionFeature> {
        self.features.iter().find(|feature| feature.key == key)
    }
}

/// A semantic operation supported by a connection type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ConnectionFeature {
    /// Domain-neutral key, for example `ai.chat` or `messaging.queues`.
    pub key: String,
    /// Runtime implementation selected by this connection, when applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
    /// Registered read-only resolver used to enumerate resources.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_resolver: Option<String>,
}

/// Generic request for connection-backed resources.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ConnectionResourceRequest {
    pub resource: String,
    #[serde(default)]
    pub arguments: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(default)]
    pub refresh: bool,
}

/// One normalized option returned by a connection-backed resolver.
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

#[derive(Debug, Clone, Copy)]
struct FeatureDefinition {
    key: &'static str,
    driver: Option<&'static str>,
    resource_resolver: Option<&'static str>,
}

const OPENAI_FEATURES: &[FeatureDefinition] = &[
    FeatureDefinition {
        key: "ai.chat",
        driver: Some("openai"),
        resource_resolver: None,
    },
    FeatureDefinition {
        key: "llm.models",
        driver: None,
        resource_resolver: Some("openai.models"),
    },
];

const AWS_FEATURES: &[FeatureDefinition] = &[
    FeatureDefinition {
        key: "ai.chat",
        driver: Some("bedrock"),
        resource_resolver: None,
    },
    FeatureDefinition {
        key: "llm.models",
        driver: None,
        resource_resolver: Some("aws.bedrock.models"),
    },
    FeatureDefinition {
        key: "messaging.queues",
        driver: None,
        resource_resolver: Some("aws.sqs.queues"),
    },
];

/// Return the safe semantic features declared by a connection integration.
///
/// The registry is intentionally independent of workflow step types. New
/// connection integrations can expose any number of runtime drivers and
/// read-only discovery resolvers without changing the connection binding.
pub fn features_for_integration(integration_id: &str) -> Vec<ConnectionFeature> {
    let definitions = match integration_id {
        "openai_api_key" => OPENAI_FEATURES,
        "aws_credentials" => AWS_FEATURES,
        _ => &[],
    };

    definitions
        .iter()
        .map(|definition| ConnectionFeature {
            key: definition.key.to_string(),
            driver: definition.driver.map(str::to_string),
            resource_resolver: definition.resource_resolver.map(str::to_string),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_declares_chat_driver_and_model_discovery() {
        let features = features_for_integration("openai_api_key");
        assert_eq!(features.len(), 2);
        assert_eq!(features[0].key, "ai.chat");
        assert_eq!(features[0].driver.as_deref(), Some("openai"));
        assert_eq!(features[1].key, "llm.models");
        assert_eq!(
            features[1].resource_resolver.as_deref(),
            Some("openai.models")
        );
    }

    #[test]
    fn generic_aws_connection_exposes_multiple_domains() {
        let features = features_for_integration("aws_credentials");
        assert_eq!(features.len(), 3);
        assert!(features.iter().any(|feature| {
            feature.key == "ai.chat" && feature.driver.as_deref() == Some("bedrock")
        }));
        assert!(features.iter().any(|feature| {
            feature.key == "messaging.queues"
                && feature.resource_resolver.as_deref() == Some("aws.sqs.queues")
        }));
    }

    #[test]
    fn unknown_integration_has_no_declared_features() {
        assert!(features_for_integration("custom").is_empty());
    }

    #[test]
    fn descriptor_feature_lookup_is_semantic() {
        let descriptor = ConnectionDescriptor {
            connection_id: "conn".into(),
            integration_id: "openai_api_key".into(),
            connection_subtype: None,
            status: ConnectionStatus::Active,
            features: features_for_integration("openai_api_key"),
            metadata: Value::Null,
        };
        assert_eq!(
            descriptor
                .feature("ai.chat")
                .and_then(|feature| feature.driver.as_deref()),
            Some("openai")
        );
    }
}
