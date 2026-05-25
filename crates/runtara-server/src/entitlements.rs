use crate::config::ConfigError;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntitlementError {
    FeatureDisabled(FeatureKey), // → ENTITLEMENT_REQUIRED
    AgentNotEnabled(String),     // → AGENT_NOT_ENABLED
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Ord, PartialOrd, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum FeatureKey {
    Reports,
    Database,
    Api,
    Mcp,
}

impl FeatureKey {
    /// Every feature key. The resolved snapshot carries a value for each.
    pub const ALL: [FeatureKey; 4] = [
        FeatureKey::Reports,
        FeatureKey::Database,
        FeatureKey::Api,
        FeatureKey::Mcp,
    ];

    /// Wire identifier — the snake_case string used in error bodies and the
    /// `features` map keys. Mirrors the serde rename so the two never drift.
    pub const fn name(self) -> &'static str {
        match self {
            FeatureKey::Reports => "reports",
            FeatureKey::Database => "database",
            FeatureKey::Api => "api",
            FeatureKey::Mcp => "mcp",
        }
    }

    /// Human-readable label used in default error messages.
    pub const fn display_name(self) -> &'static str {
        match self {
            FeatureKey::Reports => "Reports",
            FeatureKey::Database => "Database",
            FeatureKey::Api => "API access",
            FeatureKey::Mcp => "MCP",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, ToSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EntitlementLimits {
    pub max_workflows: Option<u32>,
    pub max_object_schemas: Option<u32>,
    pub max_api_keys: Option<u32>,
    pub object_model_bulk_request_limit: Option<usize>,
    pub max_concurrent_executions: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementSnapshot {
    pub tenant_id: String,
    pub pricing_tier: Tier,
    features: BTreeMap<FeatureKey, bool>,
    /// `None` = all known agent modules are allowed.
    /// `Some(set)` = exact allowlist (may be empty to disable all agents).
    pub enabled_agents: Option<BTreeSet<String>>,
    pub limits: EntitlementLimits,
    registered_agents: BTreeSet<String>,
}

impl EntitlementSnapshot {
    pub fn parse_entitlements(
        tenant_id: &str,
        runtara_pricing_tier: Option<&str>,
        runtara_entitlement_json: Option<&str>,
        runtara_entitlement_overrides_json: Option<&str>,
        registered_agents: &BTreeSet<String>,
    ) -> Result<EntitlementSnapshot, ConfigError> {
        // Tier base: the built-in baseline selected by RUNTARA_PRICING_TIER.
        // RUNTARA_ENTITLEMENTS_JSON / _OVERRIDES_JSON apply as partial diffs on top.
        let tier = get_tier(runtara_pricing_tier)?;
        let base = tier.get();
        let mut snapshot = EntitlementSnapshot {
            tenant_id: tenant_id.to_string(),
            pricing_tier: tier,
            features: base.features.into_map(),
            enabled_agents: base.enabled_agents,
            limits: base.limits,
            registered_agents: registered_agents.clone(),
        };
        if let Some(agents) = &snapshot.enabled_agents {
            validate_agents("RUNTARA_PRICING_TIER", agents, registered_agents)?;
        }

        if let Some(json) = runtara_entitlement_json {
            let layer = parse_layer("RUNTARA_ENTITLEMENTS_JSON", json)?;
            apply_layer(
                &mut snapshot,
                "RUNTARA_ENTITLEMENTS_JSON",
                registered_agents,
                layer,
            )?;
        }
        if let Some(json) = runtara_entitlement_overrides_json {
            let layer = parse_layer("RUNTARA_ENTITLEMENT_OVERRIDES_JSON", json)?;
            apply_layer(
                &mut snapshot,
                "RUNTARA_ENTITLEMENT_OVERRIDES_JSON",
                registered_agents,
                layer,
            )?;
        }

        Ok(snapshot)
    }

    pub fn is_feature_enabled(&self, feature: FeatureKey) -> bool {
        self.features.get(&feature).copied().unwrap_or(false)
    }

    pub fn require_feature(&self, feature: FeatureKey) -> Result<(), EntitlementError> {
        if self.is_feature_enabled(feature) {
            Ok(())
        } else {
            Err(EntitlementError::FeatureDisabled(feature))
        }
    }
    pub fn is_agent_enabled(&self, agent: &str) -> bool {
        // Must be a registered dispatcher module first — `enabled_agents: None`
        // means "all registered agents", not "any string you can dream up".
        self.registered_agents.contains(agent)
            && self
                .enabled_agents
                .as_ref()
                .map_or(true, |allowed| allowed.contains(agent))
    }

    pub fn require_agent(&self, agent: &str) -> Result<(), EntitlementError> {
        if self.is_agent_enabled(agent) {
            Ok(())
        } else {
            Err(EntitlementError::AgentNotEnabled(agent.to_string()))
        }
    }

    pub fn features(&self) -> &BTreeMap<FeatureKey, bool> {
        &self.features
    }

    /// Wire-shape agent allowlist: `enabled_agents` collapsed against the
    /// registered dispatcher modules. `None` ("all known") materialises to
    /// the full registered set so the frontend never sees an implicit-all
    /// sentinel.
    pub fn materialised_agents(&self) -> BTreeSet<String> {
        match &self.enabled_agents {
            Some(allowed) => allowed.clone(),
            None => self.registered_agents.clone(),
        }
    }
}

/// One partial entitlement layer, as deserialized from a `RUNTARA_ENTITLEMENT*_JSON`
/// env var. Every field is optional — an absent field inherits the layer below.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EntitlementLayer {
    features: Option<BTreeMap<FeatureKey, bool>>,
    agents: Option<BTreeSet<String>>,
    limits: Option<EntitlementLimits>,
}

/// Deserialize one entitlement layer, mapping any JSON error to a startup
/// `ConfigError` tagged with the originating env var.
fn parse_layer(env_name: &'static str, json: &str) -> Result<EntitlementLayer, ConfigError> {
    serde_json::from_str(json).map_err(|err| ConfigError::InvalidValue(env_name, err.to_string()))
}

/// Merge a parsed layer onto the running snapshot.
///
/// `features` and `limits` merge per field — only keys the layer names are
/// overridden. `agents` is a whole-field replace: an allowlist cannot be
/// element-merged, so a present `agents` (even `[]`) replaces the lower layer.
fn apply_layer(
    snapshot: &mut EntitlementSnapshot,
    env_name: &'static str,
    registered_agents: &BTreeSet<String>,
    layer: EntitlementLayer,
) -> Result<(), ConfigError> {
    if let Some(features) = layer.features {
        snapshot.features.extend(features);
    }

    if let Some(agents) = layer.agents {
        validate_agents(env_name, &agents, registered_agents)?;
        snapshot.enabled_agents = Some(agents);
    }

    if let Some(limits) = layer.limits {
        let current = &mut snapshot.limits;
        current.max_workflows = limits.max_workflows.or(current.max_workflows);
        current.max_object_schemas = limits.max_object_schemas.or(current.max_object_schemas);
        current.max_api_keys = limits.max_api_keys.or(current.max_api_keys);
        current.object_model_bulk_request_limit = limits
            .object_model_bulk_request_limit
            .or(current.object_model_bulk_request_limit);
        current.max_concurrent_executions = limits
            .max_concurrent_executions
            .or(current.max_concurrent_executions);
    }

    Ok(())
}

/// Verify a single agent id is a registered dispatcher module.
fn validate_agent(
    env_name: &'static str,
    agent: &str,
    registered_agents: &BTreeSet<String>,
) -> Result<(), ConfigError> {
    if registered_agents.contains(agent) {
        Ok(())
    } else {
        Err(ConfigError::InvalidValue(
            env_name,
            format!("unknown agent module '{agent}'"),
        ))
    }
}

/// Verify every agent id is a registered dispatcher module.
fn validate_agents(
    env_name: &'static str,
    agents: &BTreeSet<String>,
    registered_agents: &BTreeSet<String>,
) -> Result<(), ConfigError> {
    for agent in agents {
        validate_agent(env_name, agent, registered_agents)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Default,
    Starter,
    Premium,
    Enterprise,
}

impl Tier {
    fn get(&self) -> TierBase {
        match self {
            Tier::Default => TierBase::all_enabled(),
            Tier::Starter => TierBase::starter(),
            Tier::Premium => TierBase::premium(),
            Tier::Enterprise => TierBase::enterprise(),
        }
    }
}
/// The four feature flags a pricing tier must set. No `Option` — the compiler
/// guarantees a tier definition is complete; a tier cannot leave a feature unset.
struct TierFeatures {
    reports: bool,
    database: bool,
    api: bool,
    mcp: bool,
}

impl TierFeatures {
    fn into_map(self) -> BTreeMap<FeatureKey, bool> {
        BTreeMap::from([
            (FeatureKey::Reports, self.reports),
            (FeatureKey::Database, self.database),
            (FeatureKey::Api, self.api),
            (FeatureKey::Mcp, self.mcp),
        ])
    }
}

/// The complete baseline a pricing tier contributes, before any JSON layer.
struct TierBase {
    features: TierFeatures,
    enabled_agents: Option<BTreeSet<String>>,
    limits: EntitlementLimits,
}

impl TierBase {
    /// Everything on, no caps. Used when `RUNTARA_PRICING_TIER` is unset —
    /// preserves the historical local-dev default.

    fn all_enabled() -> Self {
        TierBase {
            features: TierFeatures {
                reports: true,
                database: true,
                api: true,
                mcp: true,
            },
            enabled_agents: None,
            limits: EntitlementLimits::default(),
        }
    }

    // ─────────────────────────────────────────────────────────────────────
    // PLACEHOLDER tier definitions (SYN-433). The product values for Starter
    // / Premium / Enterprise are not decided yet — the gradient below exists
    // only so tier resolution is testable against the JSON layers. Replace
    // each field with the real catalog once the product decides.
    // ─────────────────────────────────────────────────────────────────────

    fn starter() -> Self {
        TierBase {
            features: TierFeatures {
                reports: true,
                database: false,
                api: false,
                mcp: false,
            },
            enabled_agents: Some(parse_agents(&["http", "csv"])),
            limits: EntitlementLimits {
                max_workflows: Some(10),
                max_object_schemas: Some(5),
                max_api_keys: Some(2),
                object_model_bulk_request_limit: Some(100),
                max_concurrent_executions: Some(2),
            },
        }
    }

    fn premium() -> Self {
        TierBase {
            features: TierFeatures {
                reports: true,
                database: true,
                api: true,
                mcp: false,
            },
            enabled_agents: Some(parse_agents(&["http", "csv", "xml", "openai", "anthropic"])),
            limits: EntitlementLimits {
                max_workflows: Some(100),
                max_object_schemas: Some(50),
                max_api_keys: Some(10),
                object_model_bulk_request_limit: Some(1000),
                max_concurrent_executions: Some(8),
            },
        }
    }

    fn enterprise() -> Self {
        TierBase {
            features: TierFeatures {
                reports: true,
                database: true,
                api: true,
                mcp: true,
            },
            enabled_agents: None,
            limits: EntitlementLimits::default(),
        }
    }
}

/// Resolve the built-in tier baseline named by `RUNTARA_PRICING_TIER`.
///
/// An unset tier preserves the all-enabled local-dev default. An unrecognised
/// name fails startup — it is operator input, so a `ConfigError`, not a panic.
fn get_tier(tier: Option<&str>) -> Result<Tier, ConfigError> {
    match tier.map(|name| name.to_ascii_lowercase()).as_deref() {
        None => Ok(Tier::Default),
        Some("starter") => Ok(Tier::Starter),
        Some("premium") => Ok(Tier::Premium),
        Some("enterprise") => Ok(Tier::Enterprise),
        Some(other) => Err(ConfigError::InvalidValue(
            "RUNTARA_PRICING_TIER",
            format!("unknown pricing tier '{other}'"),
        )),
    }
}

pub fn parse_agents(agents: &[&str]) -> BTreeSet<String> {
    agents.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use crate::config::ConfigError;
    use crate::entitlements::FeatureKey::{Api, Database, Mcp, Reports};
    use crate::entitlements::{EntitlementError, EntitlementSnapshot, FeatureKey, Tier};
    use std::collections::BTreeSet;

    fn parse(
        runtara_pricing_tier: Option<&str>,
        runtara_entitlement_json: Option<&str>,
        runtara_entitlement_overrides_json: Option<&str>,
    ) -> Result<EntitlementSnapshot, ConfigError> {
        let agents = super::parse_agents(&["http", "csv", "xml", "openai", "anthropic"]);
        EntitlementSnapshot::parse_entitlements(
            "tenant-123",
            runtara_pricing_tier,
            runtara_entitlement_json,
            runtara_entitlement_overrides_json,
            &agents,
        )
    }

    #[test]
    fn test_feature_is_enabled() {
        let mut snap = parse(None, None, None).unwrap();
        snap.features.insert(Api, false);
        snap.features.insert(Mcp, true);

        assert!(!snap.is_feature_enabled(Api));
        assert!(snap.is_feature_enabled(Mcp));
    }

    #[test]
    fn test_agent_enabled_when_indicated() {
        let mut snap = parse(None, None, None).unwrap();
        snap.enabled_agents = Some(super::parse_agents(&["http"]));

        assert!(snap.is_agent_enabled("http"));
        assert!(!snap.is_agent_enabled("csv"));
    }

    #[test]
    fn test_agent_enabled_when_none() {
        let snap = parse(None, None, None).unwrap();

        assert!(snap.is_agent_enabled("http"));
        assert!(snap.is_agent_enabled("csv"));
    }

    #[test]
    fn test_agent_disabled_when_empty() {
        let mut snap = parse(None, None, None).unwrap();
        snap.enabled_agents = Some(BTreeSet::new());

        assert!(!snap.is_agent_enabled("http"));
        assert!(!snap.is_agent_enabled("csv"));
    }

    #[test]
    fn test_agent_disabled_when_incorrect() {
        let snap = parse(None, None, None).unwrap();

        assert!(!snap.is_agent_enabled("incorrect"));
    }

    #[test]
    fn require_feature_ok_when_enabled() {
        let snap = parse(None, None, None).unwrap();
        assert_eq!(snap.require_feature(Reports), Ok(()));
    }

    #[test]
    fn require_feature_errors_when_disabled() {
        let mut snap = parse(None, None, None).unwrap();
        snap.features.insert(Reports, false);
        assert_eq!(
            snap.require_feature(Reports),
            Err(EntitlementError::FeatureDisabled(Reports)),
        );
    }

    #[test]
    fn require_agent_ok_when_allowed() {
        // enabled_agents = None → every registered agent is allowed.
        let snap = parse(None, None, None).unwrap();
        assert_eq!(snap.require_agent("http"), Ok(()));
    }

    #[test]
    fn require_agent_errors_when_not_in_allowlist() {
        // Starter: enabled_agents = Some({http, csv}); openai is registered but not allowed.
        let snap = parse(Some("starter"), None, None).unwrap();
        assert_eq!(
            snap.require_agent("openai"),
            Err(EntitlementError::AgentNotEnabled("openai".to_string())),
        );
    }

    #[test]
    fn require_agent_errors_when_unregistered() {
        // enabled_agents = None (all registered allowed), but "fake" isn't registered.
        let snap = parse(None, None, None).unwrap();
        assert_eq!(
            snap.require_agent("fake"),
            Err(EntitlementError::AgentNotEnabled("fake".to_string())),
        );
    }

    #[test]
    fn missing_env_defaults_to_all_enabled() {
        let snap = parse(None, None, None).unwrap();
        for key in FeatureKey::ALL {
            assert!(snap.is_feature_enabled(key), "{key:?} should default on");
        }
        assert_eq!(snap.enabled_agents, None);
        assert_eq!(snap.pricing_tier, Tier::Default);
    }

    #[test]
    fn missing_pricing_tier_falls_back_to_default() {
        let snap = parse(None, None, None).unwrap();
        let default_tier = Tier::Default.get();
        let snap_from_default_tier = EntitlementSnapshot {
            tenant_id: snap.tenant_id.clone(),
            pricing_tier: Tier::Default,
            features: default_tier.features.into_map(),
            enabled_agents: default_tier.enabled_agents.clone(),
            limits: default_tier.limits,
            registered_agents: snap.registered_agents.clone(),
        };
        assert_eq!(snap, snap_from_default_tier);
    }

    #[test]
    fn valid_json_parses_and_applies() {
        let json = r#"{"features":{"reports":false},"agents":["http"],"limits":{"maxApiKeys":5}}"#;
        let snap = parse(None, Some(json), None).unwrap();
        assert!(!snap.is_feature_enabled(Reports));
        assert!(snap.is_feature_enabled(Database)); // untouched → tier default
        assert_eq!(snap.enabled_agents, Some(super::parse_agents(&["http"])));
        assert_eq!(snap.limits.max_api_keys, Some(5));
    }

    #[test]
    fn unknown_feature_key_is_rejected() {
        assert!(parse(None, Some(r#"{"features":{"reportz":true}}"#), None).is_err());
    }

    #[test]
    fn non_boolean_feature_value_is_rejected() {
        assert!(parse(None, Some(r#"{"features":{"reports":"yes"}}"#), None).is_err());
    }

    #[test]
    fn unknown_top_level_field_is_rejected() {
        assert!(parse(None, Some(r#"{"agentz":["http"]}"#), None).is_err());
    }

    #[test]
    fn invalid_json_is_rejected() {
        assert!(parse(None, Some("{not json"), None).is_err());
    }

    #[test]
    fn unknown_agent_is_rejected() {
        assert!(parse(None, Some(r#"{"agents":["http","nosuchagent"]}"#), None).is_err());
    }

    #[test]
    fn empty_agents_array_disables_all() {
        let snap = parse(None, Some(r#"{"agents":[]}"#), None).unwrap();
        assert_eq!(snap.enabled_agents, Some(BTreeSet::new()));
    }

    #[test]
    fn negative_limit_is_rejected() {
        assert!(parse(None, Some(r#"{"limits":{"maxApiKeys":-1}}"#), None).is_err());
    }

    #[test]
    fn overflowing_limit_is_rejected() {
        assert!(
            parse(
                None,
                Some(r#"{"limits":{"maxApiKeys":99999999999999}}"#),
                None
            )
            .is_err()
        );
    }

    #[test]
    fn overrides_take_precedence_per_field() {
        let entitlements =
            r#"{"features":{"reports":false,"api":false},"limits":{"maxApiKeys":5}}"#;
        let overrides = r#"{"features":{"reports":true},"limits":{"maxApiKeys":20}}"#;
        let snap = parse(Some("enterprise"), Some(entitlements), Some(overrides)).unwrap();
        assert!(snap.is_feature_enabled(Reports));
        assert!(snap.is_feature_enabled(Database));
        assert!(!snap.is_feature_enabled(Api));
        assert!(snap.is_feature_enabled(Mcp));
        assert_eq!(snap.limits.max_api_keys, Some(20)); // override wins
    }

    #[test]
    fn partial_override_keeps_lower_layers() {
        let entitlements = r#"{"features":{"mcp":false},"agents":["http"]}"#;
        let overrides = r#"{"limits":{"maxWorkflows":50}}"#;
        let snap = parse(Some("enterprise"), Some(entitlements), Some(overrides)).unwrap();
        assert!(!snap.is_feature_enabled(Mcp)); // survives the limits-only override
        assert_eq!(snap.enabled_agents, Some(super::parse_agents(&["http"]))); // survives too
        assert_eq!(snap.limits.max_workflows, Some(50));
    }

    #[test]
    fn agents_field_replaces_rather_than_merges() {
        let entitlements = r#"{"agents":["http","csv"]}"#;
        let overrides = r#"{"agents":["xml"]}"#;
        let snap = parse(Some("enterprise"), Some(entitlements), Some(overrides)).unwrap();
        assert_eq!(snap.enabled_agents, Some(super::parse_agents(&["xml"])));
    }

    #[test]
    fn starter_tier_baseline() {
        let snap = parse(Some("starter"), None, None).unwrap();
        assert!(snap.is_feature_enabled(Reports));
        assert!(!snap.is_feature_enabled(Database));
        assert!(!snap.is_feature_enabled(Api));
        assert!(!snap.is_feature_enabled(Mcp));
        assert_eq!(
            snap.enabled_agents,
            Some(super::parse_agents(&["http", "csv"]))
        );
        assert_eq!(snap.limits.max_api_keys, Some(2));
        assert_eq!(snap.pricing_tier, Tier::Starter);
    }

    #[test]
    fn premium_tier_baseline() {
        let snap = parse(Some("premium"), None, None).unwrap();
        assert!(snap.is_feature_enabled(Reports));
        assert!(snap.is_feature_enabled(Database));
        assert!(snap.is_feature_enabled(Api));
        assert!(!snap.is_feature_enabled(Mcp));
        for agent in ["http", "csv", "xml", "openai", "anthropic"] {
            assert!(
                snap.enabled_agents
                    .as_ref()
                    .map_or(true, |allowed| allowed.contains(agent)),
                "premium tier should allow agent '{agent}', got {:?}",
                snap.enabled_agents,
            );
        }
        assert_eq!(snap.limits.max_api_keys, Some(10));
    }

    #[test]
    fn enterprise_tier_has_no_limits() {
        let snap = parse(Some("enterprise"), None, None).unwrap();
        assert!(snap.is_feature_enabled(Reports));
        assert!(snap.is_feature_enabled(Database));
        assert!(snap.is_feature_enabled(Api));
        assert!(snap.is_feature_enabled(Mcp));
        assert_eq!(snap.enabled_agents, None);
        assert_eq!(snap.limits.max_workflows, None);
        assert_eq!(snap.limits.max_api_keys, None);
    }

    #[test]
    fn tier_name_is_case_insensitive() {
        let snap = parse(Some("STARTER"), None, None).unwrap();
        assert!(!snap.is_feature_enabled(Database));
        assert_eq!(snap.pricing_tier, Tier::Starter);
    }

    #[test]
    fn unknown_tier_is_rejected() {
        assert!(parse(Some("pro"), None, None).is_err());
    }

    #[test]
    fn entitlements_json_overrides_tier() {
        // Starter has database/api off; the JSON layer turns database on.
        let snap = parse(
            Some("starter"),
            Some(r#"{"features":{"database":true},"limits":{"maxApiKeys":50}}"#),
            None,
        )
        .unwrap();
        assert!(snap.is_feature_enabled(Database));
        assert!(snap.is_feature_enabled(Reports));
        assert!(!snap.is_feature_enabled(Api));
        assert!(!snap.is_feature_enabled(Mcp));
        assert_eq!(snap.limits.max_api_keys, Some(50));
        assert_eq!(snap.limits.max_workflows, Some(10));
    }

    #[test]
    fn overrides_json_beats_tier_and_entitlements() {
        // tier starter: database off → entitlements: on → overrides: off
        let snap = parse(
            Some("starter"),
            Some(r#"{"features":{"database":true}}"#),
            Some(r#"{"features":{"database":false}}"#),
        )
        .unwrap();
        assert!(!snap.is_feature_enabled(Database));
        assert!(snap.is_feature_enabled(Reports));
        assert!(!snap.is_feature_enabled(Api));
        assert!(!snap.is_feature_enabled(Mcp));
    }
}
