// Copyright (C) 2025 SyncMyOrders Sp. z o.o.
// SPDX-License-Identifier: AGPL-3.0-or-later
//! Compatibility lookup for `(integration_id, default_for)` pairs.
//!
//! Connection records store `default_for` as a list of strings — usually
//! an agent id like `slack`, sometimes a virtual platform bucket like
//! `object_storage`. The connection service needs to validate that the
//! `integration_id` of a connection actually belongs to the bucket the
//! caller is claiming it as default for.
//!
//! Resolving the answer requires knowing which `integration_ids` each
//! "bucket" supports — which used to come from `runtara_agents::registry`
//! at validation time. That coupled the connection service to the agent
//! crate. After the runtime-discovery migration, the agent set is loaded
//! once at server boot into an `AgentCatalog`; this module captures the
//! catalog's `default_for → integration_ids` view into a small value
//! type, and the connection service consumes only that.

use std::collections::HashMap;

/// Platform-level virtual default-for bucket: aggregates the storage
/// integration ids that the file-storage UI / API treats as
/// interchangeable. Not an agent — kept here so it doesn't leak into
/// service code that's otherwise agent-agnostic.
pub const OBJECT_STORAGE_DEFAULT_FOR: &str = "object_storage";

/// Integration ids that satisfy the `object_storage` default-for bucket.
const OBJECT_STORAGE_INTEGRATION_IDS: &[&str] = &["s3_compatible", "azure_blob_storage"];

/// Maps each `default_for` bucket (an agent id, or a virtual bucket like
/// `object_storage`) to the integration ids that satisfy it. Constructed
/// once at server boot from the runtime [`AgentCatalog`] plus the static
/// platform buckets, then passed by `Arc` to every `ConnectionService`.
#[derive(Debug, Clone, Default)]
pub struct IntegrationCompatibility {
    by_default_for: HashMap<String, Vec<String>>,
}

impl IntegrationCompatibility {
    /// Build from a raw map. Useful in tests where the caller wants to
    /// inline a fixture without going through `AgentCatalog`.
    pub fn new(by_default_for: HashMap<String, Vec<String>>) -> Self {
        let mut me = Self { by_default_for };
        me.install_platform_buckets();
        me
    }

    /// Build from a runtime [`AgentCatalog`]. Each agent contributes one
    /// entry keyed by its id; the platform-level virtual buckets are
    /// merged in on top.
    pub fn from_catalog(catalog: &runtara_dsl::agent_meta::AgentCatalog) -> Self {
        let mut by_default_for: HashMap<String, Vec<String>> = catalog
            .agents()
            .iter()
            .map(|a| (a.id.clone(), a.integration_ids.clone()))
            .collect();
        // Install platform buckets last so they always win in case an agent
        // happens to share the same id (none do today; defensive).
        for (key, ids) in platform_buckets() {
            by_default_for.insert(key.to_string(), ids);
        }
        Self { by_default_for }
    }

    fn install_platform_buckets(&mut self) {
        for (key, ids) in platform_buckets() {
            self.by_default_for.entry(key.to_string()).or_insert(ids);
        }
    }

    /// True if `integration_id` is one of the ids declared for the
    /// `default_for` bucket.
    pub fn is_compatible(&self, integration_id: &str, default_for: &str) -> bool {
        self.by_default_for
            .get(default_for)
            .map(|ids| ids.iter().any(|id| id == integration_id))
            .unwrap_or(false)
    }
}

fn platform_buckets() -> Vec<(&'static str, Vec<String>)> {
    vec![(
        OBJECT_STORAGE_DEFAULT_FOR,
        OBJECT_STORAGE_INTEGRATION_IDS
            .iter()
            .map(|s| (*s).to_string())
            .collect(),
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_storage_bucket_is_always_installed() {
        let compat = IntegrationCompatibility::new(HashMap::new());
        assert!(compat.is_compatible("s3_compatible", OBJECT_STORAGE_DEFAULT_FOR));
        assert!(compat.is_compatible("azure_blob_storage", OBJECT_STORAGE_DEFAULT_FOR));
        assert!(!compat.is_compatible("slack_oauth", OBJECT_STORAGE_DEFAULT_FOR));
    }

    #[test]
    fn from_catalog_maps_each_agent_id_to_its_integrations() {
        use runtara_dsl::agent_meta::{AgentCatalog, AgentInfo, CapabilityInfo, FieldTypeInfo};

        let slack = AgentInfo {
            id: "slack".into(),
            name: "Slack".into(),
            description: "".into(),
            has_side_effects: true,
            supports_connections: true,
            integration_ids: vec!["slack_oauth".into()],
            capabilities: vec![CapabilityInfo {
                id: "send".into(),
                name: "send".into(),
                display_name: None,
                description: None,
                input_type: "SendInput".into(),
                inputs: vec![],
                output: FieldTypeInfo {
                    type_name: "SendOutput".into(),
                    format: None,
                    display_name: None,
                    description: None,
                    items: None,
                    fields: None,
                    nullable: false,
                },
                has_side_effects: true,
                is_idempotent: false,
                rate_limited: true,
                compensation_hint: None,
                known_errors: vec![],
                tags: vec![],
            }],
        };
        let catalog = AgentCatalog::from_agents(vec![slack]);
        let compat = IntegrationCompatibility::from_catalog(&catalog);

        assert!(compat.is_compatible("slack_oauth", "slack"));
        assert!(!compat.is_compatible("slack_oauth", "mailgun"));
        // Platform bucket still works on a catalog-derived map.
        assert!(compat.is_compatible("s3_compatible", OBJECT_STORAGE_DEFAULT_FOR));
    }
}
