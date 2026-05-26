//! REST entitlement gates — see `docs/entitlements.md`.
//!
//! - **Feature gates (`require_reports` etc.)**: per-feature middleware
//!   that short-circuits with a 403 + stable `code` when the feature is off.
//!   Mounted on the smallest sub-router that exactly contains the routes
//!   surfacing the feature, so the gate is visible at the mount point and
//!   cannot leak to neighbouring routes.
//! - **API-key auth bypass guard ([`api_key_auth_guard`])**: post-auth
//!   middleware that rejects API-key-authenticated requests on *every*
//!   tenant route when `api` is disabled, regardless of which route is hit.
//!   Session-cookie / JWT users on the same routes are unaffected — that's
//!   the entire point. Wraps every authenticated route group; mounted
//!   between `authenticate` (outer) and the feature gates (inner).
//!
//! - **3.4 — per-agent allowlist** ([`agent_decision`], [`walk_graph_for_agents`]):
//!   reject handlers / workflow steps referencing a module not in the tenant's
//!   `enabled_agents` allowlist.
//! - **3.6 — numeric tier limits** ([`limit_decision`], [`effective_limit`]):
//!   count-before-create gates for `maxApiKeys` / `maxObjectSchemas` /
//!   `maxWorkflows`, plus `min(infra, tier)` composition for the two
//!   infra-shared caps (`objectModelBulkRequestLimit`,
//!   `maxConcurrentExecutions`). Used directly by service handlers — there is
//!   no middleware layer, since the check needs a tenant-scoped row count.

use axum::{extract::Request, middleware::Next, response::IntoResponse, response::Response};

use crate::auth::{AuthContext, AuthMethod};
use crate::entitlement_error::EntitlementDenial;
use crate::entitlements::{EntitlementSnapshot, FeatureKey};

/// Pure gate decision. Returns the denial to surface (with stable `code`)
/// when the snapshot says the feature is off; `Ok(())` lets the request
/// proceed. Pulled out as a free function so unit tests can exercise it
/// against constructed snapshots without booting the global `OnceLock<Config>`.
pub fn gate_decision(
    snapshot: &EntitlementSnapshot,
    feature: FeatureKey,
) -> Result<(), EntitlementDenial> {
    snapshot
        .require_feature(feature)
        .map_err(EntitlementDenial::from)
}

/// Glue: read the global snapshot, run [`gate_decision`], short-circuit with a
/// 403 or hand off to the inner handler chain. Kept thin so the testable
/// decision logic lives in `gate_decision`.
async fn require_feature(feature: FeatureKey, req: Request, next: Next) -> Response {
    match gate_decision(crate::config::entitlements(), feature) {
        Ok(()) => next.run(req).await,
        Err(denial) => denial.into_response(),
    }
}

pub async fn require_reports(req: Request, next: Next) -> Response {
    require_feature(FeatureKey::Reports, req, next).await
}

pub async fn require_database(req: Request, next: Next) -> Response {
    require_feature(FeatureKey::Database, req, next).await
}

pub async fn require_api(req: Request, next: Next) -> Response {
    require_feature(FeatureKey::Api, req, next).await
}

pub async fn require_mcp(req: Request, next: Next) -> Response {
    require_feature(FeatureKey::Mcp, req, next).await
}

/// Pure decision for the per-agent allowlist check.
///
/// Returns `AGENT_NOT_ENABLED` if the snapshot rejects the agent module —
/// either because it isn't a registered dispatcher module or because the
/// tenant's `enabled_agents` allowlist doesn't include it.
///
/// Mirrors [`gate_decision`] / [`api_key_decision`]: pure, no global state,
/// so unit tests can exercise it against constructed snapshots.
pub fn agent_decision(
    snapshot: &EntitlementSnapshot,
    module_id: &str,
) -> Result<(), EntitlementDenial> {
    snapshot
        .require_agent(module_id)
        .map_err(EntitlementDenial::from)
}

/// Walk an `ExecutionGraph` and return the first `AGENT_NOT_ENABLED` denial
/// for any `AgentStep` whose `agent_id` is not allowed by the snapshot, or
/// `Ok(())` if every agent step references an allowed module.
///
/// Recurses into nested subgraphs (`Split.subgraph`, `While.subgraph`,
/// `WaitForSignal.on_wait`) so workflows with deeply nested agents are not
/// silently let through.
pub fn walk_graph_for_agents(
    snapshot: &EntitlementSnapshot,
    graph: &runtara_dsl::ExecutionGraph,
) -> Result<(), EntitlementDenial> {
    for step in graph.steps.values() {
        match step {
            runtara_dsl::Step::Agent(agent_step) => {
                agent_decision(snapshot, &agent_step.agent_id)?;
            }
            runtara_dsl::Step::Split(split) => {
                walk_graph_for_agents(snapshot, &split.subgraph)?;
            }
            runtara_dsl::Step::While(w) => {
                walk_graph_for_agents(snapshot, &w.subgraph)?;
            }
            runtara_dsl::Step::WaitForSignal(s) => {
                if let Some(on_wait) = &s.on_wait {
                    walk_graph_for_agents(snapshot, on_wait)?;
                }
            }
            // Other step kinds carry no agent module reference: Finish,
            // Conditional, Switch, EmbedWorkflow, Log, Error, Filter,
            // GroupBy, Delay, AiAgent (LLM-driven; gated by provider, not
            // by the `enabled_agents` allowlist — left for a follow-up
            // if/when LLM providers join the per-agent gate).
            _ => {}
        }
    }
    Ok(())
}

/// Compose an infra-shared limit with the tenant's tier cap, using the
/// stricter of the two. Per docs/entitlements.md § Limit Composition:
/// `effective_limit = min(configured_infra_limit, entitlement_limit)`.
///
/// `None` from the entitlement side means "no tenant cap" — infrastructure
/// retains full control, mirroring the historical behavior before tier
/// limits existed.
pub fn effective_limit(infra: usize, entitlement: Option<usize>) -> usize {
    match entitlement {
        Some(tier) => infra.min(tier),
        None => infra,
    }
}

/// Pure decision for a count-before-create tier limit. Returns
/// `ENTITLEMENT_LIMIT_EXCEEDED` when the tenant already holds `current` items
/// of a kind capped at `max`. `None` for `max` means no tier cap → always Ok.
///
/// The caller passes the **current count** (rows already owned by the
/// tenant), not the post-insert count, so the check is
/// `current >= max → reject` (a create would push the total to `max + 1`).
/// `limit_name` is the stable camelCase wire identifier (e.g. `"maxApiKeys"`).
pub fn limit_decision(
    current: u64,
    max: Option<u32>,
    limit_name: &'static str,
) -> Result<(), EntitlementDenial> {
    match max {
        None => Ok(()),
        Some(cap) if current >= u64::from(cap) => Err(EntitlementDenial::LimitExceeded {
            limit: limit_name,
            maximum: u64::from(cap),
        }),
        Some(_) => Ok(()),
    }
}

/// Pure decision for the API-key auth bypass guard.
///
/// Returns `Ok(())` for non-API-key auth methods (JWT, session, unauthenticated
/// in-process calls) regardless of the `api` feature — those callers are not
/// what the gate is about. For `AuthMethod::ApiKey`, defers to the snapshot:
/// if `api` is enabled the request continues; otherwise we surface the
/// standard `ENTITLEMENT_REQUIRED` denial for the `api` feature.
///
/// Pulled out as a free function so unit tests can cover every
/// (auth_method × feature_state) combination without booting the global
/// `OnceLock<Config>`.
pub fn api_key_decision(
    snapshot: &EntitlementSnapshot,
    auth_method: &AuthMethod,
) -> Result<(), EntitlementDenial> {
    match auth_method {
        AuthMethod::ApiKey => gate_decision(snapshot, FeatureKey::Api),
        AuthMethod::Jwt | AuthMethod::Unauthenticated => Ok(()),
    }
}

/// Post-auth guard that denies API-key-authenticated requests on every
/// tenant route when `api` is disabled.
///
/// Must run **after** the auth middleware that populates `AuthContext` in
/// request extensions; if the extension is missing we let the request through
/// rather than 403'ing on a downstream failure mode (auth misconfiguration
/// surfaces elsewhere).
pub async fn api_key_auth_guard(req: Request, next: Next) -> Response {
    if let Some(ctx) = req.extensions().get::<AuthContext>().cloned()
        && let Err(denial) = api_key_decision(crate::config::entitlements(), &ctx.auth_method)
    {
        return denial.into_response();
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    //! These tests exercise the pure [`gate_decision`] function against
    //! constructed snapshots. The Axum glue (`require_*`) is a one-line
    //! delegate; its end-to-end HTTP behavior depends on the global
    //! `OnceLock<Config>` and is better covered by a future server-fixture
    //! integration test.

    use super::*;

    use std::collections::BTreeSet;

    use crate::entitlement_error::codes;
    use crate::entitlements::{EntitlementSnapshot, FeatureKey, parse_agents};

    fn registered_agents() -> BTreeSet<String> {
        parse_agents(&["http", "csv"])
    }

    fn snapshot_with(
        pricing_tier: Option<&str>,
        entitlements_json: Option<&str>,
    ) -> EntitlementSnapshot {
        EntitlementSnapshot::parse_entitlements(
            "tenant-test",
            pricing_tier,
            entitlements_json,
            None,
            &registered_agents(),
        )
        .expect("snapshot parses")
    }

    // ── default snapshot: every feature on → no denials ─────────────────

    #[test]
    fn default_snapshot_allows_every_feature() {
        let snap = snapshot_with(None, None);
        for feature in FeatureKey::ALL {
            assert!(
                gate_decision(&snap, feature).is_ok(),
                "expected gate_decision to pass for {feature:?}"
            );
        }
    }

    // ── disabled features → denial carries correct code + feature name ──

    #[test]
    fn disabled_reports_yields_entitlement_required_denial() {
        let snap = snapshot_with(None, Some(r#"{"features":{"reports":false}}"#));
        let denial = gate_decision(&snap, FeatureKey::Reports).expect_err("should deny");
        assert_eq!(denial.code(), codes::ENTITLEMENT_REQUIRED);
        let body = denial.json_body();
        assert_eq!(body["feature"], "reports");
    }

    #[test]
    fn disabled_database_yields_entitlement_required_denial() {
        let snap = snapshot_with(None, Some(r#"{"features":{"database":false}}"#));
        let denial = gate_decision(&snap, FeatureKey::Database).expect_err("should deny");
        assert_eq!(denial.code(), codes::ENTITLEMENT_REQUIRED);
        assert_eq!(denial.json_body()["feature"], "database");
    }

    #[test]
    fn disabled_api_yields_entitlement_required_denial() {
        let snap = snapshot_with(None, Some(r#"{"features":{"api":false}}"#));
        let denial = gate_decision(&snap, FeatureKey::Api).expect_err("should deny");
        assert_eq!(denial.code(), codes::ENTITLEMENT_REQUIRED);
        assert_eq!(denial.json_body()["feature"], "api");
    }

    #[test]
    fn disabled_mcp_yields_entitlement_required_denial() {
        let snap = snapshot_with(None, Some(r#"{"features":{"mcp":false}}"#));
        let denial = gate_decision(&snap, FeatureKey::Mcp).expect_err("should deny");
        assert_eq!(denial.code(), codes::ENTITLEMENT_REQUIRED);
        assert_eq!(denial.json_body()["feature"], "mcp");
    }

    // ── disabling one feature doesn't affect siblings ───────────────────

    #[test]
    fn disabling_one_feature_leaves_others_passing() {
        // `reports` off but `database`, `api`, `mcp` remain default-on.
        let snap = snapshot_with(None, Some(r#"{"features":{"reports":false}}"#));
        assert!(gate_decision(&snap, FeatureKey::Reports).is_err());
        assert!(gate_decision(&snap, FeatureKey::Database).is_ok());
        assert!(gate_decision(&snap, FeatureKey::Api).is_ok());
        assert!(gate_decision(&snap, FeatureKey::Mcp).is_ok());
    }

    // ── tier-level disablement is honoured the same way ─────────────────

    #[test]
    fn starter_tier_denies_database_api_mcp_via_gate() {
        // Starter (per the placeholder tier catalog in entitlements.rs)
        // has reports on; database/api/mcp off.
        let snap = snapshot_with(Some("starter"), None);
        assert!(gate_decision(&snap, FeatureKey::Reports).is_ok());
        assert!(gate_decision(&snap, FeatureKey::Database).is_err());
        assert!(gate_decision(&snap, FeatureKey::Api).is_err());
        assert!(gate_decision(&snap, FeatureKey::Mcp).is_err());
    }

    // ── denial response: confirms IntoResponse path stays 403 + JSON ────

    #[tokio::test]
    async fn denial_renders_as_403_with_stable_code() {
        let snap = snapshot_with(None, Some(r#"{"features":{"reports":false}}"#));
        let denial = gate_decision(&snap, FeatureKey::Reports).expect_err("should deny");
        let response = denial.into_response();

        assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .expect("body bytes");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json body");
        assert_eq!(body["code"], codes::ENTITLEMENT_REQUIRED);
        assert_eq!(body["feature"], "reports");
    }

    // ────────────────────────────────────────────────────────────────────
    // API-key auth bypass guard.
    //
    // Matrix: (auth_method × `api` feature state) → expected decision.
    // The control case is the third row — `Jwt` users on a tenant with
    // `api` off must NOT be denied; only ApiKey auth triggers the guard.
    // ────────────────────────────────────────────────────────────────────

    use crate::auth::AuthMethod;

    fn snapshot_api_off() -> EntitlementSnapshot {
        snapshot_with(None, Some(r#"{"features":{"api":false}}"#))
    }

    fn snapshot_api_on() -> EntitlementSnapshot {
        snapshot_with(None, None)
    }

    #[test]
    fn api_key_with_api_disabled_denies_with_entitlement_required() {
        let snap = snapshot_api_off();
        let denial =
            api_key_decision(&snap, &AuthMethod::ApiKey).expect_err("should deny api-key auth");
        assert_eq!(denial.code(), codes::ENTITLEMENT_REQUIRED);
        assert_eq!(denial.json_body()["feature"], "api");
    }

    #[test]
    fn api_key_with_api_enabled_passes_through() {
        let snap = snapshot_api_on();
        assert!(api_key_decision(&snap, &AuthMethod::ApiKey).is_ok());
    }

    #[test]
    fn jwt_with_api_disabled_still_passes_through() {
        // ★ Control case from docs/entitlements.md:450 — the entire purpose
        // of the guard is that JWT/session callers are unaffected by `api`.
        let snap = snapshot_api_off();
        assert!(api_key_decision(&snap, &AuthMethod::Jwt).is_ok());
    }

    #[test]
    fn jwt_with_api_enabled_passes_through() {
        let snap = snapshot_api_on();
        assert!(api_key_decision(&snap, &AuthMethod::Jwt).is_ok());
    }

    #[test]
    fn unauthenticated_in_process_calls_pass_through_regardless_of_api() {
        // In-process MCP / trust-proxy calls carry `Unauthenticated` and must
        // not be denied by this guard — they were already trusted upstream
        // and the guard targets external automation, not internal traffic.
        for snap in [snapshot_api_off(), snapshot_api_on()] {
            assert!(api_key_decision(&snap, &AuthMethod::Unauthenticated).is_ok());
        }
    }

    #[test]
    fn api_key_decision_only_inspects_api_feature() {
        // If another feature is off but `api` is on, ApiKey auth still passes.
        // Confirms the guard is exactly one boolean check, not a confused fold
        // over the whole snapshot.
        let snap = snapshot_with(
            None,
            Some(r#"{"features":{"reports":false,"database":false,"mcp":false}}"#),
        );
        assert!(api_key_decision(&snap, &AuthMethod::ApiKey).is_ok());
    }

    // ────────────────────────────────────────────────────────────────────
    // Per-agent allowlist + workflow graph walk.
    //
    // Graph fixtures are built from JSON literals via `serde_json::from_value`
    // so the tests stay readable and don't drift if the DSL types grow new
    // fields. We only assert on the gate's behaviour, not on the DSL shape.
    // ────────────────────────────────────────────────────────────────────

    use runtara_dsl::ExecutionGraph;

    // ── agent_decision ──────────────────────────────────────────────────

    #[test]
    fn agent_decision_allows_registered_when_no_explicit_allowlist() {
        // Default snapshot: enabled_agents = None → every registered agent
        // is allowed. `http` and `csv` are in registered_agents().
        let snap = snapshot_with(None, None);
        assert!(agent_decision(&snap, "http").is_ok());
        assert!(agent_decision(&snap, "csv").is_ok());
    }

    #[test]
    fn agent_decision_denies_unregistered_agents() {
        // `nosuch` isn't a registered dispatcher module — even with no
        // allowlist, the snapshot rejects unknown agent ids.
        let snap = snapshot_with(None, None);
        let denial = agent_decision(&snap, "nosuch").expect_err("unregistered must deny");
        assert_eq!(denial.code(), codes::AGENT_NOT_ENABLED);
        assert_eq!(denial.json_body()["agent"], "nosuch");
    }

    #[test]
    fn agent_decision_denies_allowed_but_not_in_explicit_allowlist() {
        // enabled_agents = ["http"]. `csv` is registered but not in this
        // tenant's allowlist → denied with AGENT_NOT_ENABLED.
        let snap = snapshot_with(None, Some(r#"{"agents":["http"]}"#));
        assert!(agent_decision(&snap, "http").is_ok());
        let denial = agent_decision(&snap, "csv").expect_err("csv must be denied");
        assert_eq!(denial.code(), codes::AGENT_NOT_ENABLED);
        assert_eq!(denial.json_body()["agent"], "csv");
    }

    // ── walk_graph_for_agents ───────────────────────────────────────────

    /// Deserialize an ExecutionGraph from a JSON literal. Panics if the
    /// fixture is malformed — that's a test-author bug, not a runtime case.
    fn parse_graph(value: serde_json::Value) -> ExecutionGraph {
        serde_json::from_value(value).expect("test fixture parses as ExecutionGraph")
    }

    #[test]
    fn graph_with_only_allowed_agents_passes() {
        let snap = snapshot_with(None, Some(r#"{"agents":["http","csv"]}"#));
        let graph = parse_graph(serde_json::json!({
            "entryPoint": "s1",
            "steps": {
                "s1": {"stepType": "Agent", "id": "s1", "agentId": "http", "capabilityId": "request"},
                "s2": {"stepType": "Agent", "id": "s2", "agentId": "csv", "capabilityId": "parse"}
            }
        }));
        assert!(walk_graph_for_agents(&snap, &graph).is_ok());
    }

    #[test]
    fn graph_with_disallowed_agent_is_rejected() {
        let snap = snapshot_with(None, Some(r#"{"agents":["http"]}"#));
        let graph = parse_graph(serde_json::json!({
            "entryPoint": "s1",
            "steps": {
                "s1": {"stepType": "Agent", "id": "s1", "agentId": "http", "capabilityId": "request"},
                "s2": {"stepType": "Agent", "id": "s2", "agentId": "csv", "capabilityId": "parse"}
            }
        }));
        let denial = walk_graph_for_agents(&snap, &graph).expect_err("csv step must fail walk");
        assert_eq!(denial.code(), codes::AGENT_NOT_ENABLED);
        assert_eq!(denial.json_body()["agent"], "csv");
    }

    #[test]
    fn graph_walk_recurses_into_split_subgraph() {
        // Disallowed agent buried inside a Split's subgraph — the walk
        // must descend, not just check top-level steps.
        let snap = snapshot_with(None, Some(r#"{"agents":["http"]}"#));
        let graph = parse_graph(serde_json::json!({
            "entryPoint": "split",
            "steps": {
                "split": {
                    "stepType": "Split",
                    "id": "split",
                    "subgraph": {
                        "entryPoint": "inner",
                        "steps": {
                            "inner": {
                                "stepType": "Agent",
                                "id": "inner",
                                "agentId": "csv",
                                "capabilityId": "parse"
                            }
                        }
                    }
                }
            }
        }));
        let denial = walk_graph_for_agents(&snap, &graph)
            .expect_err("inner csv must be reached by recursion");
        assert_eq!(denial.code(), codes::AGENT_NOT_ENABLED);
        assert_eq!(denial.json_body()["agent"], "csv");
    }

    #[test]
    fn graph_walk_recurses_into_while_subgraph() {
        let snap = snapshot_with(None, Some(r#"{"agents":["http"]}"#));
        let graph = parse_graph(serde_json::json!({
            "entryPoint": "loop",
            "steps": {
                "loop": {
                    "stepType": "While",
                    "id": "loop",
                    "condition": {
                        "type": "operation",
                        "op": "EQ",
                        "arguments": [
                            {"valueType": "immediate", "value": 1},
                            {"valueType": "immediate", "value": 1}
                        ]
                    },
                    "subgraph": {
                        "entryPoint": "inner",
                        "steps": {
                            "inner": {
                                "stepType": "Agent",
                                "id": "inner",
                                "agentId": "csv",
                                "capabilityId": "parse"
                            }
                        }
                    }
                }
            }
        }));
        assert!(walk_graph_for_agents(&snap, &graph).is_err());
    }

    #[test]
    fn graph_without_agent_steps_passes_even_when_allowlist_is_empty() {
        // No AgentStep in the graph → nothing to gate, walk is a no-op.
        let snap = snapshot_with(None, Some(r#"{"agents":[]}"#));
        let graph = parse_graph(serde_json::json!({
            "entryPoint": "done",
            "steps": {
                "done": {
                    "stepType": "Finish",
                    "id": "done"
                }
            }
        }));
        assert!(walk_graph_for_agents(&snap, &graph).is_ok());
    }

    // ────────────────────────────────────────────────────────────────────
    // Numeric tier limits.
    // ────────────────────────────────────────────────────────────────────

    // ── effective_limit ─────────────────────────────────────────────────

    #[test]
    fn effective_limit_uses_infra_when_tier_unset() {
        // `None` from the entitlement → infra alone governs (legacy behavior).
        assert_eq!(effective_limit(1000, None), 1000);
    }

    #[test]
    fn effective_limit_uses_tier_when_stricter() {
        // Tier cap below the infra cap → tier wins.
        assert_eq!(effective_limit(1000, Some(100)), 100);
    }

    #[test]
    fn effective_limit_uses_infra_when_tier_looser() {
        // Tier cap above the infra cap → infra still wins. The composition
        // rule is `min`, so pricing tiers can never raise the infra ceiling.
        assert_eq!(effective_limit(50, Some(1000)), 50);
    }

    #[test]
    fn effective_limit_zero_tier_collapses_to_zero() {
        // A tier explicitly capping at 0 effectively disables the resource.
        assert_eq!(effective_limit(1000, Some(0)), 0);
    }

    // ── limit_decision ──────────────────────────────────────────────────

    #[test]
    fn limit_decision_passes_when_no_cap() {
        assert!(limit_decision(99_999, None, "maxApiKeys").is_ok());
    }

    #[test]
    fn limit_decision_passes_below_cap() {
        // Current count < max → an insert would put us at current + 1 ≤ max.
        assert!(limit_decision(4, Some(5), "maxApiKeys").is_ok());
    }

    #[test]
    fn limit_decision_rejects_at_cap() {
        // current == max → an insert would exceed → reject.
        let denial = limit_decision(5, Some(5), "maxApiKeys").expect_err("at-cap must reject");
        assert_eq!(denial.code(), codes::ENTITLEMENT_LIMIT_EXCEEDED);
        let body = denial.json_body();
        assert_eq!(body["limit"], "maxApiKeys");
        assert_eq!(body["maximum"], 5);
    }

    #[test]
    fn limit_decision_rejects_over_cap() {
        // Pathological case: somehow we already have more than the cap (e.g.
        // tier shrank). Reject any further inserts.
        let denial = limit_decision(10, Some(5), "maxWorkflows").expect_err("over-cap must reject");
        assert_eq!(denial.code(), codes::ENTITLEMENT_LIMIT_EXCEEDED);
        assert_eq!(denial.json_body()["limit"], "maxWorkflows");
    }

    #[test]
    fn limit_decision_with_zero_cap_rejects_immediately() {
        // A tier with `Some(0)` means "this resource is fully disabled".
        // Even the very first insert must fail.
        let denial = limit_decision(0, Some(0), "maxObjectSchemas")
            .expect_err("zero cap means no inserts allowed");
        assert_eq!(denial.code(), codes::ENTITLEMENT_LIMIT_EXCEEDED);
        assert_eq!(denial.json_body()["maximum"], 0);
    }

    #[test]
    fn limit_decision_handles_large_current_count() {
        // Sanity: a count over u32::MAX still works (we hold current as u64).
        // The cap is u32 so the comparison up-casts cleanly.
        let huge = u64::from(u32::MAX) + 10;
        let denial = limit_decision(huge, Some(u32::MAX), "maxWorkflows").expect_err("must reject");
        assert_eq!(denial.json_body()["maximum"], u64::from(u32::MAX));
    }
}
