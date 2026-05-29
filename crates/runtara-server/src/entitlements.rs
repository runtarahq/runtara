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
                .is_none_or(|allowed| allowed.contains(agent))
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

    /// Build the operator-facing summary of the resolved snapshot.
    ///
    /// Pure function so unit tests can pin down the exact strings emitted
    /// at startup without spinning up `tracing`. The returned struct is
    /// the single source of truth for the fields that
    /// [`log_summary`](Self::log_summary) hands to `tracing::info!`.
    pub fn summarize(&self) -> EntitlementSummary {
        let (mut enabled, mut disabled): (Vec<&str>, Vec<&str>) = (Vec::new(), Vec::new());
        for (key, on) in &self.features {
            if *on {
                enabled.push(key.name());
            } else {
                disabled.push(key.name());
            }
        }
        // BTreeMap iteration is already sorted by key — explicit join keeps
        // the field stable to grep against.
        EntitlementSummary {
            tenant_id: self.tenant_id.clone(),
            pricing_tier: self.pricing_tier.clone(),
            features_enabled: enabled.join(","),
            features_disabled: disabled.join(","),
            // Whether the operator explicitly listed an allowlist. `false`
            // means `enabled_agents = None`, i.e. every registered module
            // is implicitly enabled — the historical default. Operators
            // who can't reproduce the materialised list from their env
            // alone want this signal.
            agents_explicit: self.enabled_agents.is_some(),
            agents_allowlist_size: self.materialised_agents().len(),
            max_workflows: self.limits.max_workflows,
            max_object_schemas: self.limits.max_object_schemas,
            max_api_keys: self.limits.max_api_keys,
            object_model_bulk_request_limit: self.limits.object_model_bulk_request_limit,
            max_concurrent_executions: self.limits.max_concurrent_executions,
        }
    }

    /// Emit one structured `tracing::info!` line describing the active
    /// snapshot. Called once during server startup so operators can verify
    /// what's actually enforced without re-deriving it from env. Logs at
    /// `info` level — denial events use `warn` (see Phase 6.2).
    pub fn log_summary(&self) {
        let s = self.summarize();
        tracing::info!(
            tenant_id = %s.tenant_id,
            pricing_tier = ?s.pricing_tier,
            features_enabled = %s.features_enabled,
            features_disabled = %s.features_disabled,
            agents_explicit = s.agents_explicit,
            agents_allowlist_size = s.agents_allowlist_size,
            max_workflows = ?s.max_workflows,
            max_object_schemas = ?s.max_object_schemas,
            max_api_keys = ?s.max_api_keys,
            object_model_bulk_request_limit = ?s.object_model_bulk_request_limit,
            max_concurrent_executions = ?s.max_concurrent_executions,
            "entitlement snapshot resolved"
        );
    }

    /// Pure predicate: `true` when `mcp` is enabled but `api` is disabled.
    ///
    /// This combination is *valid* (boot succeeds) but behaves surprisingly:
    /// the hosted MCP transport at `/mcp/*` is only reachable via API-key
    /// auth, and the API-key bypass guard (`api_key_auth_guard`) rejects
    /// every API-key-authenticated request when `api` is off — *before* the
    /// `mcp` transport gate ever runs. So a tenant reading "mcp: true,
    /// api: false" expects MCP to work, but every hosted MCP client gets
    /// `ENTITLEMENT_REQUIRED`. See SYN-433 Finding 5.
    ///
    /// Pulled out as a pure function so the detection is unit-testable
    /// against constructed snapshots without spinning up `tracing`.
    pub fn mcp_unreachable_without_api(&self) -> bool {
        self.is_feature_enabled(FeatureKey::Mcp) && !self.is_feature_enabled(FeatureKey::Api)
    }

    /// Emit operator `warn` lines for entitlement combinations that boot
    /// cleanly but behave in surprising ways. Called once at startup, right
    /// after [`log_summary`](Self::log_summary).
    ///
    /// Currently covers the `mcp=true` + `api=false` collision (SYN-433
    /// Finding 5). Kept separate from `log_summary` so the always-on `info`
    /// summary stays uncluttered and each risky combination is one greppable
    /// `warn` line an operator sees during the deploy that introduced it.
    pub fn warn_risky_combinations(&self) {
        if self.mcp_unreachable_without_api() {
            tracing::warn!(
                tenant_id = %self.tenant_id,
                feature_mcp = true,
                feature_api = false,
                "Entitlement combination mcp=true, api=false: hosted \
                 MCP-over-HTTP requires API-key auth, which the api gate \
                 blocks. MCP appears enabled in the snapshot but every \
                 request from an API-key MCP client will fail with \
                 ENTITLEMENT_REQUIRED. Either enable `api`, or treat `mcp` \
                 as effectively disabled for hosted clients."
            );
        }
    }
}

/// Plain-data shape of the startup entitlement summary. Returned by
/// [`EntitlementSnapshot::summarize`] and consumed by
/// [`EntitlementSnapshot::log_summary`]. Kept separate from the snapshot
/// itself so the log-formatting choices (CSV vs. `Vec`, `Option<u32>` vs.
/// `String "—"`, etc.) stay in one place and are independently testable.
#[derive(Debug, Clone, PartialEq)]
pub struct EntitlementSummary {
    pub tenant_id: String,
    pub pricing_tier: Tier,
    /// Comma-separated list of feature keys (snake_case) with value `true`.
    pub features_enabled: String,
    /// Complement of `features_enabled`.
    pub features_disabled: String,
    /// `true` when the operator explicitly set an `agents` allowlist
    /// (including `agents = []`); `false` when the snapshot inherits the
    /// implicit-all default.
    pub agents_explicit: bool,
    /// Size of the materialised allowlist — i.e. how many agent modules
    /// this process will actually accept. With `agents_explicit = false`
    /// this is the count of registered modules.
    pub agents_allowlist_size: usize,
    pub max_workflows: Option<u32>,
    pub max_object_schemas: Option<u32>,
    pub max_api_keys: Option<u32>,
    pub object_model_bulk_request_limit: Option<usize>,
    pub max_concurrent_executions: Option<usize>,
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
    Default, // "RUNTARA_PRICING_TIER: default" is incorrect way, it shouldn't be configured at all
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
            enabled_agents: None,
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
                    .is_none_or(|allowed| allowed.contains(agent)),
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

    #[test]
    fn summarize_reports_all_enabled_default() {
        let snap = parse(None, None, None).unwrap();
        let s = snap.summarize();

        assert_eq!(s.tenant_id, "tenant-123");
        assert_eq!(s.pricing_tier, Tier::Default);
        // BTreeMap iteration is sorted, so the CSV order is stable.
        assert_eq!(s.features_enabled, "reports,database,api,mcp");
        assert_eq!(s.features_disabled, "");
        // Default snapshot has `enabled_agents = None` → implicit-all.
        assert!(!s.agents_explicit);
        // ... and materialises to the registered set, which the test
        // fixture seeds with five agents.
        assert_eq!(s.agents_allowlist_size, 5);
        // No tier caps in the default.
        assert_eq!(s.max_workflows, None);
        assert_eq!(s.max_api_keys, None);
    }

    #[test]
    fn summarize_reports_restrictive_snapshot() {
        // Reports off, database on, explicit two-agent allowlist, a tier
        // cap on api keys.
        let snap = parse(
            None,
            Some(
                r#"{"features":{"reports":false},"agents":["http","csv"],"limits":{"maxApiKeys":5}}"#,
            ),
            None,
        )
        .unwrap();
        let s = snap.summarize();

        assert_eq!(s.features_enabled, "database,api,mcp");
        assert_eq!(s.features_disabled, "reports");
        assert!(s.agents_explicit);
        assert_eq!(s.agents_allowlist_size, 2);
        assert_eq!(s.max_api_keys, Some(5));
        // Limits not mentioned in JSON stay None.
        assert_eq!(s.max_workflows, None);
    }

    #[test]
    fn summarize_distinguishes_explicit_empty_from_implicit_all() {
        // `agents = []` is the explicit "deny everything" allowlist. The
        // summary must mark it as explicit and report zero size — operators
        // need to be able to tell at a glance that a tenant has zero agents
        // available vs. the implicit-all default.
        let snap = parse(None, Some(r#"{"agents":[]}"#), None).unwrap();
        let s = snap.summarize();

        assert!(s.agents_explicit);
        assert_eq!(s.agents_allowlist_size, 0);
    }

    // ── mcp_unreachable_without_api (SYN-433 Finding 5) ─────────────────

    #[test]
    fn mcp_without_api_is_flagged() {
        // mcp=true, api=false → the surprising combination that silently
        // breaks hosted MCP-over-HTTP.
        let snap = parse(None, Some(r#"{"features":{"mcp":true,"api":false}}"#), None).unwrap();
        assert!(
            snap.mcp_unreachable_without_api(),
            "mcp=true + api=false must be flagged"
        );
    }

    #[test]
    fn mcp_with_api_is_not_flagged() {
        // Both on (default) → no warning. This is the healthy combination.
        let snap = parse(None, None, None).unwrap();
        assert!(snap.is_feature_enabled(Mcp));
        assert!(snap.is_feature_enabled(Api));
        assert!(!snap.mcp_unreachable_without_api());
    }

    #[test]
    fn api_without_mcp_is_not_flagged() {
        // api on, mcp off → nothing surprising; API works, MCP is just off.
        let snap = parse(None, Some(r#"{"features":{"mcp":false,"api":true}}"#), None).unwrap();
        assert!(!snap.mcp_unreachable_without_api());
    }

    #[test]
    fn both_mcp_and_api_off_is_not_flagged() {
        // Both off → MCP is intentionally disabled, no false reassurance to
        // warn about. The warning is specifically about mcp *appearing*
        // enabled while being unreachable.
        let snap = parse(
            None,
            Some(r#"{"features":{"mcp":false,"api":false}}"#),
            None,
        )
        .unwrap();
        assert!(!snap.mcp_unreachable_without_api());
    }
}
