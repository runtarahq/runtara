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
    // Registered dispatcher modules are kebab-canonical; DSL graphs may still
    // carry legacy snake_case agent ids ("object_model").
    snapshot
        .require_agent(&runtara_dsl::agent_meta::canonical_agent_id(module_id))
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
            // Other step kinds carry no agent module reference of their
            // own: Finish, Conditional, Switch, Log, Error, Filter,
            // GroupBy, Delay, AiAgent (LLM-driven; gated by provider, not
            // by the `enabled_agents` allowlist — left for a follow-up
            // if/when LLM providers join the per-agent gate).
            //
            // EmbedWorkflow is deliberately excluded from that list: its
            // *referenced child graph* can absolutely contain Agent steps,
            // and skipping it here would silently let a forbidden agent
            // through via an embedded child. It isn't walked from inside
            // this function because resolving the child requires an async
            // database load this pure/sync walk can't perform. Callers
            // that have resolved the EmbedWorkflow closure (e.g. for
            // structural validation) must additionally check every child
            // graph via [`walk_closure_for_agents`] — walking the root
            // alone is not sufficient.
            _ => {}
        }
    }
    Ok(())
}

/// Walk the root graph and every already-resolved `EmbedWorkflow` child in
/// its closure, returning the first `AGENT_NOT_ENABLED` denial found
/// anywhere — root or any (grand)child — or `Ok(())` if every graph in the
/// closure only references allowed agents.
///
/// [`walk_graph_for_agents`] alone only ever sees the graph it's handed: it
/// does not (and cannot, being pure/sync) resolve `EmbedWorkflow`
/// references, which requires an async database load. Callers that have
/// already resolved the closure — e.g. for `validate_workflow_closure`'s
/// structural checks — should pass that same resolved list here so a
/// forbidden agent buried in a saved child can't slip past the allowlist.
pub fn walk_closure_for_agents<'a>(
    snapshot: &EntitlementSnapshot,
    root: &runtara_dsl::ExecutionGraph,
    children: impl IntoIterator<Item = &'a runtara_dsl::ExecutionGraph>,
) -> Result<(), EntitlementDenial> {
    walk_graph_for_agents(snapshot, root)?;
    for child in children {
        walk_graph_for_agents(snapshot, child)?;
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

/// Pure decision for the per-request bulk-size cap
/// (`objectModelBulkRequestLimit`). Different semantics from
/// [`limit_decision`]: the caller passes the **request size** (how many
/// items this one call wants to operate on), and the cap is honored
/// inclusively — `requested == cap` is allowed; `requested > cap` denies.
///
/// `None` for the entitlement cap means no tenant-side cap → always Ok;
/// the store crate's infra-side `bulk_request_limit` then becomes the only
/// thing that can fire (and surfaces as the legacy validation error,
/// acceptable for the no-entitlement-cap case).
///
/// SYN-433 Finding 2: the wire shape produced when this decision denies is
/// the documented `ENTITLEMENT_LIMIT_EXCEEDED` body, not the generic
/// validation error the store crate has historically produced.
pub fn bulk_size_decision(
    snapshot: &EntitlementSnapshot,
    requested: usize,
) -> Result<(), EntitlementDenial> {
    if let Some(cap) = snapshot.limits.object_model_bulk_request_limit
        && requested > cap
    {
        return Err(EntitlementDenial::LimitExceeded {
            limit: "objectModelBulkRequestLimit",
            maximum: cap as u64,
        });
    }
    Ok(())
}

/// Pure decision for the per-tenant in-flight execution cap
/// (`maxConcurrentExecutions`). The caller passes the **observed in-flight
/// count** (executions enqueued and not yet terminal); the decision is
/// `in_flight >= cap → reject` — the next intake would push the tenant
/// past `cap` simultaneous runs.
///
/// `effective_limit(infra, snapshot.limits.max_concurrent_executions)` is
/// the composed cap. `None` from the entitlement means no tenant-side cap;
/// the infra value (currently `num_cpus::get() * 32` by default, settable
/// via `MAX_CONCURRENT_EXECUTIONS`) is still applied as the upper bound.
///
/// SYN-433 Finding 1: before this decision, `maxConcurrentExecutions` was
/// parsed from env and stored in `Config` but **never consulted** by any
/// code path — setting it had zero observable effect. The wire shape
/// produced on denial is the documented `ENTITLEMENT_LIMIT_EXCEEDED` body
/// (same `code` callers already switch on for `maxWorkflows` /
/// `maxObjectSchemas` / `maxApiKeys`).
pub fn concurrent_executions_decision(
    snapshot: &EntitlementSnapshot,
    in_flight: u64,
    infra_cap: usize,
) -> Result<(), EntitlementDenial> {
    let cap = effective_limit(infra_cap, snapshot.limits.max_concurrent_executions);
    if in_flight >= cap as u64 {
        return Err(EntitlementDenial::LimitExceeded {
            limit: "maxConcurrentExecutions",
            maximum: cap as u64,
        });
    }
    Ok(())
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
    fn agent_decision_folds_snake_case_ids_to_kebab() {
        // Registered dispatcher modules are kebab-canonical
        // ("object-model"), but DSL graphs may carry legacy snake_case ids
        // ("object_model"). The decision must fold before matching, or
        // every legacy workflow gets a spurious AGENT_NOT_ENABLED.
        let snap = EntitlementSnapshot::parse_entitlements(
            "tenant-test",
            None,
            None,
            None,
            &parse_agents(&["object-model"]),
        )
        .expect("snapshot parses");
        assert!(agent_decision(&snap, "object_model").is_ok());
        assert!(agent_decision(&snap, "object-model").is_ok());
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

    // ── walk_closure_for_agents ─────────────────────────────────────────

    #[test]
    fn closure_walk_rejects_disallowed_agent_hidden_in_embedded_child() {
        // Root is clean; the forbidden agent only exists inside an
        // EmbedWorkflow child's resolved graph — walk_graph_for_agents(root)
        // alone would miss it.
        let snap = snapshot_with(None, Some(r#"{"agents":["http"]}"#));
        let root = parse_graph(serde_json::json!({
            "entryPoint": "s1",
            "steps": {
                "s1": {"stepType": "Agent", "id": "s1", "agentId": "http", "capabilityId": "request"},
                "embed": {"stepType": "EmbedWorkflow", "id": "embed", "childWorkflowId": "child-wf", "childVersion": "latest"}
            }
        }));
        let child = parse_graph(serde_json::json!({
            "entryPoint": "inner",
            "steps": {
                "inner": {"stepType": "Agent", "id": "inner", "agentId": "csv", "capabilityId": "parse"}
            }
        }));

        assert!(
            walk_graph_for_agents(&snap, &root).is_ok(),
            "sanity check: root alone must look clean, or this test doesn't reproduce the bug"
        );

        let denial = walk_closure_for_agents(&snap, &root, [&child])
            .expect_err("csv step inside the embedded child must be reached");
        assert_eq!(denial.code(), codes::AGENT_NOT_ENABLED);
        assert_eq!(denial.json_body()["agent"], "csv");
    }

    #[test]
    fn closure_walk_passes_when_root_and_every_child_are_allowed() {
        let snap = snapshot_with(None, Some(r#"{"agents":["http","csv"]}"#));
        let root = parse_graph(serde_json::json!({
            "entryPoint": "s1",
            "steps": {
                "s1": {"stepType": "Agent", "id": "s1", "agentId": "http", "capabilityId": "request"}
            }
        }));
        let child = parse_graph(serde_json::json!({
            "entryPoint": "inner",
            "steps": {
                "inner": {"stepType": "Agent", "id": "inner", "agentId": "csv", "capabilityId": "parse"}
            }
        }));
        assert!(walk_closure_for_agents(&snap, &root, [&child]).is_ok());
    }

    #[test]
    fn closure_walk_with_no_children_matches_root_only_walk() {
        // Empty children list must behave exactly like walk_graph_for_agents
        // alone — no surprises for callers with no EmbedWorkflow steps.
        let snap = snapshot_with(None, Some(r#"{"agents":["http"]}"#));
        let root = parse_graph(serde_json::json!({
            "entryPoint": "s1",
            "steps": {
                "s1": {"stepType": "Agent", "id": "s1", "agentId": "csv", "capabilityId": "parse"}
            }
        }));
        let denial = walk_closure_for_agents(&snap, &root, std::iter::empty())
            .expect_err("csv must still be denied with zero children");
        assert_eq!(denial.code(), codes::AGENT_NOT_ENABLED);
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

    // ── bulk_size_decision ──────────────────────────────────────────────
    //
    // SYN-433 Finding 2: per-request bulk caps must return the documented
    // ENTITLEMENT_LIMIT_EXCEEDED shape. These tests pin the boundary
    // semantics: `requested == cap` allowed, `requested > cap` denied,
    // `None` cap means no entitlement-side gating.

    #[test]
    fn bulk_size_decision_passes_when_no_cap_set() {
        // Default snapshot: object_model_bulk_request_limit = None → never gates.
        let snap = snapshot_with(None, None);
        assert!(bulk_size_decision(&snap, 0).is_ok());
        assert!(bulk_size_decision(&snap, 1_000_000).is_ok());
    }

    #[test]
    fn bulk_size_decision_allows_exact_cap_inclusive() {
        // Boundary: `requested == cap` must succeed. Production verified at
        // both cap=1 (round 1) and cap=2 (round 2b).
        let snap = snapshot_with(
            None,
            Some(r#"{"limits":{"objectModelBulkRequestLimit":2}}"#),
        );
        assert!(bulk_size_decision(&snap, 2).is_ok());
    }

    #[test]
    fn bulk_size_decision_denies_one_over_cap() {
        // Boundary: `requested == cap + 1` must deny with the documented shape.
        let snap = snapshot_with(
            None,
            Some(r#"{"limits":{"objectModelBulkRequestLimit":2}}"#),
        );
        let denial = bulk_size_decision(&snap, 3).expect_err("3 > cap of 2 must deny");
        assert_eq!(denial.code(), codes::ENTITLEMENT_LIMIT_EXCEEDED);
        let body = denial.json_body();
        assert_eq!(body["limit"], "objectModelBulkRequestLimit");
        assert_eq!(body["maximum"], 2);
    }

    #[test]
    fn bulk_size_decision_at_cap_one_denies_two_rows() {
        // The exact round-1 production scenario: cap=1, 1 row OK, 2 rows denied.
        let snap = snapshot_with(
            None,
            Some(r#"{"limits":{"objectModelBulkRequestLimit":1}}"#),
        );
        assert!(bulk_size_decision(&snap, 0).is_ok());
        assert!(bulk_size_decision(&snap, 1).is_ok());
        let denial = bulk_size_decision(&snap, 2).expect_err("2 rows under cap=1 must deny");
        assert_eq!(denial.code(), codes::ENTITLEMENT_LIMIT_EXCEEDED);
        assert_eq!(denial.json_body()["maximum"], 1);
    }

    #[test]
    fn bulk_size_decision_zero_request_always_passes() {
        // Empty bulk requests are degenerate but shouldn't be denied —
        // downstream validation handles "you sent nothing" with its own error.
        let snap = snapshot_with(
            None,
            Some(r#"{"limits":{"objectModelBulkRequestLimit":1}}"#),
        );
        assert!(bulk_size_decision(&snap, 0).is_ok());
    }

    // ── concurrent_executions_decision ─────────────────────────────────
    //
    // SYN-433 Finding 1: enforce maxConcurrentExecutions at intake. Before
    // these tests, the cap was never read by any code path.
    //
    // Semantics: `in_flight >= cap → reject` (next intake would breach).
    // Composition: `effective_limit(infra, tier)` is the stricter of the
    // two; entitlement `None` falls back to infra alone.

    /// Default-config infra cap used in tests. Production uses
    /// `num_cpus::get() * 32`, but tests need a deterministic value.
    const TEST_INFRA_CAP: usize = 8;

    #[test]
    fn concurrent_executions_decision_passes_when_no_cap_set() {
        // entitlement = None → infra cap alone applies; in_flight below it OK.
        let snap = snapshot_with(None, None);
        assert!(
            concurrent_executions_decision(&snap, 0, TEST_INFRA_CAP).is_ok(),
            "empty in-flight under default snapshot must pass"
        );
        assert!(
            concurrent_executions_decision(&snap, (TEST_INFRA_CAP as u64) - 1, TEST_INFRA_CAP)
                .is_ok()
        );
    }

    #[test]
    fn concurrent_executions_decision_rejects_at_infra_cap_even_without_tier() {
        // Pure infra-only cap fires at infra_cap. No tier needed.
        let snap = snapshot_with(None, None);
        let denial = concurrent_executions_decision(&snap, TEST_INFRA_CAP as u64, TEST_INFRA_CAP)
            .expect_err("at infra cap must reject");
        assert_eq!(denial.code(), codes::ENTITLEMENT_LIMIT_EXCEEDED);
        assert_eq!(denial.json_body()["limit"], "maxConcurrentExecutions");
        assert_eq!(denial.json_body()["maximum"], TEST_INFRA_CAP as u64);
    }

    #[test]
    fn concurrent_executions_decision_uses_stricter_of_infra_and_tier() {
        // infra=8, tier=2 → effective cap = 2. At 2 in flight, deny.
        let snap = snapshot_with(None, Some(r#"{"limits":{"maxConcurrentExecutions":2}}"#));
        assert!(concurrent_executions_decision(&snap, 1, TEST_INFRA_CAP).is_ok());
        let denial = concurrent_executions_decision(&snap, 2, TEST_INFRA_CAP)
            .expect_err("tier cap=2 must fire before infra cap=8");
        assert_eq!(denial.code(), codes::ENTITLEMENT_LIMIT_EXCEEDED);
        assert_eq!(
            denial.json_body()["maximum"],
            2,
            "denial body must reflect the stricter (tier) cap, not infra"
        );
    }

    #[test]
    fn concurrent_executions_decision_uses_infra_when_tier_is_higher() {
        // infra=8, tier=100 → effective cap = 8. Tier wider than infra
        // doesn't raise the cap (entitlement can only narrow, never widen).
        let snap = snapshot_with(None, Some(r#"{"limits":{"maxConcurrentExecutions":100}}"#));
        let denial = concurrent_executions_decision(&snap, TEST_INFRA_CAP as u64, TEST_INFRA_CAP)
            .expect_err("infra cap=8 must fire even though tier=100");
        assert_eq!(
            denial.json_body()["maximum"],
            TEST_INFRA_CAP as u64,
            "denial body must reflect the stricter (infra) cap"
        );
    }

    #[test]
    fn concurrent_executions_decision_rejects_at_cap_one() {
        // Production round-1 scenario: tier=1 means one concurrent execution.
        // First intake (in_flight=0) OK; second (in_flight=1) denied.
        let snap = snapshot_with(None, Some(r#"{"limits":{"maxConcurrentExecutions":1}}"#));
        assert!(concurrent_executions_decision(&snap, 0, TEST_INFRA_CAP).is_ok());
        let denial = concurrent_executions_decision(&snap, 1, TEST_INFRA_CAP)
            .expect_err("at cap=1 the 2nd execution must deny");
        assert_eq!(denial.json_body()["maximum"], 1);
    }

    #[test]
    fn concurrent_executions_decision_zero_cap_denies_even_first_intake() {
        // `maxConcurrentExecutions: 0` composes to an effective cap of 0
        // and must deny the very first intake (in_flight == 0), the same
        // way `limit_decision` treats `Some(0)` as "fully disabled". The
        // Valkey gate mirrors this with its own `cap == 0` short-circuit.
        let snap = snapshot_with(None, Some(r#"{"limits":{"maxConcurrentExecutions":0}}"#));
        let denial = concurrent_executions_decision(&snap, 0, TEST_INFRA_CAP)
            .expect_err("zero cap means no executions allowed");
        assert_eq!(denial.code(), codes::ENTITLEMENT_LIMIT_EXCEEDED);
        assert_eq!(denial.json_body()["limit"], "maxConcurrentExecutions");
        assert_eq!(denial.json_body()["maximum"], 0);
    }

    #[test]
    fn concurrent_executions_decision_rejects_when_drifted_over_cap() {
        // Self-healing reconciler may briefly observe in_flight > cap
        // (race window or zombie entries that haven't aged out yet). Don't
        // crash; reject normally with the correct cap in the body.
        let snap = snapshot_with(None, Some(r#"{"limits":{"maxConcurrentExecutions":2}}"#));
        let denial = concurrent_executions_decision(&snap, 5, TEST_INFRA_CAP)
            .expect_err("drift must still deny");
        assert_eq!(denial.json_body()["maximum"], 2);
    }

    #[tokio::test]
    async fn concurrent_executions_decision_denial_renders_as_403_with_stable_code() {
        // Same IntoResponse path as the other limit denials.
        let snap = snapshot_with(None, Some(r#"{"limits":{"maxConcurrentExecutions":2}}"#));
        let denial =
            concurrent_executions_decision(&snap, 2, TEST_INFRA_CAP).expect_err("at cap must deny");
        let response = denial.into_response();
        assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .expect("body bytes");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json body");
        assert_eq!(body["code"], codes::ENTITLEMENT_LIMIT_EXCEEDED);
        assert_eq!(body["limit"], "maxConcurrentExecutions");
        assert_eq!(body["maximum"], 2);
    }

    #[tokio::test]
    async fn bulk_size_decision_denial_renders_as_403_with_stable_code() {
        // Confirms the denial flows through IntoResponse with the documented
        // status + body shape — same path the handler-side helper takes.
        let snap = snapshot_with(
            None,
            Some(r#"{"limits":{"objectModelBulkRequestLimit":1}}"#),
        );
        let denial = bulk_size_decision(&snap, 5).expect_err("over-cap must deny");
        let response = denial.into_response();

        assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .expect("body bytes");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json body");
        assert_eq!(body["code"], codes::ENTITLEMENT_LIMIT_EXCEEDED);
        assert_eq!(body["limit"], "objectModelBulkRequestLimit");
        assert_eq!(body["maximum"], 1);
    }

    // ────────────────────────────────────────────────────────────────────
    // Route-layer composition (HTTP-level).
    //
    // The `require_*` glue functions read the global `OnceLock<Config>`,
    // which makes them awkward to drive in tests. The composition pattern
    // they form together with `route_layer` IS testable though — these
    // tests mount a snapshot-parameterised closure with the same shape as
    // `require_feature`, exercise it via `tower::ServiceExt::oneshot`, and
    // verify that a request hitting a gated path short-circuits with the
    // documented denial body before the handler runs.
    // ────────────────────────────────────────────────────────────────────

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::post;
    use axum::{Router, middleware::from_fn};
    use tower::ServiceExt;

    /// Mirror of `require_feature` that takes the snapshot as a parameter
    /// instead of reading the global. Tests use this to assert the route
    /// composition emits the expected denial body. Production code stays
    /// on the global-reading variant.
    fn make_test_gate(
        snapshot: EntitlementSnapshot,
        feature: FeatureKey,
    ) -> impl Clone
    + Send
    + Sync
    + 'static
    + Fn(Request<Body>, Next) -> futures::future::BoxFuture<'static, Response> {
        use futures::FutureExt;
        move |req: Request<Body>, next: Next| {
            let snapshot = snapshot.clone();
            async move {
                match gate_decision(&snapshot, feature) {
                    Ok(()) => next.run(req).await,
                    Err(d) => d.into_response(),
                }
            }
            .boxed()
        }
    }

    fn dummy_handler() -> axum::response::Response {
        // If the gate lets the request through, the response body is "ok" —
        // tests assert this is *not* what they see when the gate denies.
        axum::response::Response::new(Body::from("ok"))
    }

    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
            .await
            .expect("read body");
        serde_json::from_slice(&bytes).expect("parse JSON body")
    }

    #[tokio::test]
    async fn database_gate_short_circuits_internal_object_model_path() {
        // Snapshot: `database` disabled. Any /api/internal/object-model/*
        // path must 403 with ENTITLEMENT_REQUIRED before reaching the
        // handler. This is the Phase 5.1 wiring under test.
        let snapshot = snapshot_with(None, Some(r#"{"features":{"database":false}}"#));
        let gate = make_test_gate(snapshot, FeatureKey::Database);

        let app = Router::new()
            .route(
                "/api/internal/object-model/instances/query",
                post(|| async { dummy_handler() }),
            )
            .route_layer(from_fn(gate));

        let request = Request::post("/api/internal/object-model/instances/query")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(request).await.unwrap();

        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = body_json(resp).await;
        assert_eq!(body["code"], codes::ENTITLEMENT_REQUIRED);
        assert_eq!(body["feature"], "database");
    }

    #[tokio::test]
    async fn database_gate_short_circuits_internal_sql_path() {
        // The workflow raw-SQL routes live in the same internal router group
        // and must inherit the same gate: `database` disabled → 403
        // ENTITLEMENT_REQUIRED before the handler (and any SQL) runs.
        let snapshot = snapshot_with(None, Some(r#"{"features":{"database":false}}"#));
        let gate = make_test_gate(snapshot, FeatureKey::Database);

        let app = Router::new()
            .route(
                "/api/internal/object-model/sql/query",
                post(|| async { dummy_handler() }),
            )
            .route(
                "/api/internal/object-model/sql/execute",
                post(|| async { dummy_handler() }),
            )
            .route_layer(from_fn(gate));

        for path in [
            "/api/internal/object-model/sql/query",
            "/api/internal/object-model/sql/execute",
        ] {
            let request = Request::post(path).body(Body::empty()).unwrap();
            let resp = app.clone().oneshot(request).await.unwrap();

            assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{path}");
            let body = body_json(resp).await;
            assert_eq!(body["code"], codes::ENTITLEMENT_REQUIRED, "{path}");
            assert_eq!(body["feature"], "database", "{path}");
        }
    }

    #[tokio::test]
    async fn database_gate_lets_request_through_when_enabled() {
        // Control case: same wiring, same path, but the feature is on. The
        // request must reach the handler — proving the gate is the only
        // thing standing between request and execution.
        let snapshot = snapshot_with(None, Some(r#"{"features":{"database":true}}"#));
        let gate = make_test_gate(snapshot, FeatureKey::Database);

        let app = Router::new()
            .route(
                "/api/internal/object-model/instances/query",
                post(|| async { dummy_handler() }),
            )
            .route_layer(from_fn(gate));

        let request = Request::post("/api/internal/object-model/instances/query")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(request).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1024)
            .await
            .expect("read body");
        assert_eq!(&bytes[..], b"ok");
    }

    // ── api_key_auth_guard composition ─────────────────────────────────
    //
    // Mirror of the database-gate tests above, but for the post-auth guard
    // that rejects ApiKey callers on every tenant route when `api` is off.
    // The guard reads `AuthContext` from request extensions; tests inject
    // a synthetic AuthContext via a wrapping `from_fn` middleware so we
    // don't have to spin up the real auth chain.

    fn inject_auth_context(
        ctx: AuthContext,
    ) -> impl Clone
    + Send
    + Sync
    + 'static
    + Fn(Request<Body>, Next) -> futures::future::BoxFuture<'static, Response> {
        use futures::FutureExt;
        move |mut req: Request<Body>, next: Next| {
            req.extensions_mut().insert(ctx.clone());
            async move { next.run(req).await }.boxed()
        }
    }

    fn make_test_api_key_guard(
        snapshot: EntitlementSnapshot,
    ) -> impl Clone
    + Send
    + Sync
    + 'static
    + Fn(Request<Body>, Next) -> futures::future::BoxFuture<'static, Response> {
        use futures::FutureExt;
        move |req: Request<Body>, next: Next| {
            let snapshot = snapshot.clone();
            async move {
                if let Some(ctx) = req.extensions().get::<AuthContext>().cloned()
                    && let Err(denial) = api_key_decision(&snapshot, &ctx.auth_method)
                {
                    return denial.into_response();
                }
                next.run(req).await
            }
            .boxed()
        }
    }

    fn ctx_with(method: AuthMethod) -> AuthContext {
        AuthContext::new("tenant-test".to_string(), "user-test".to_string(), method)
    }

    #[tokio::test]
    async fn api_key_guard_denies_api_key_when_api_disabled() {
        // The Connections gap that prompted this test: an ApiKey caller on
        // a tenant with `api=false` must be rejected with ENTITLEMENT_REQUIRED
        // regardless of which tenant sub-router serves the request.
        let snapshot = snapshot_with(None, Some(r#"{"features":{"api":false}}"#));
        let guard = make_test_api_key_guard(snapshot);
        let inject = inject_auth_context(ctx_with(AuthMethod::ApiKey));

        let app = Router::new()
            .route(
                "/api/runtime/connections",
                post(|| async { dummy_handler() }),
            )
            .route_layer(from_fn(guard))
            .route_layer(from_fn(inject));

        let req = Request::post("/api/runtime/connections")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = body_json(resp).await;
        assert_eq!(body["code"], codes::ENTITLEMENT_REQUIRED);
        assert_eq!(body["feature"], "api");
    }

    #[tokio::test]
    async fn api_key_guard_allows_jwt_callers_when_api_disabled() {
        // Control case: same `api=false` snapshot, but the request is JWT-
        // authenticated. Session/OAuth users on the same routes must NOT be
        // denied — that's the whole point of the bypass guard scoping.
        let snapshot = snapshot_with(None, Some(r#"{"features":{"api":false}}"#));
        let guard = make_test_api_key_guard(snapshot);
        let inject = inject_auth_context(ctx_with(AuthMethod::Jwt));

        let app = Router::new()
            .route(
                "/api/runtime/connections",
                post(|| async { dummy_handler() }),
            )
            .route_layer(from_fn(guard))
            .route_layer(from_fn(inject));

        let req = Request::post("/api/runtime/connections")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn api_key_guard_allows_api_key_when_api_enabled() {
        // Sanity: the guard must not be a hard "ApiKey denied" — it gates on
        // the feature, not on the auth method itself.
        let snapshot = snapshot_with(None, Some(r#"{"features":{"api":true}}"#));
        let guard = make_test_api_key_guard(snapshot);
        let inject = inject_auth_context(ctx_with(AuthMethod::ApiKey));

        let app = Router::new()
            .route(
                "/api/runtime/connections",
                post(|| async { dummy_handler() }),
            )
            .route_layer(from_fn(guard))
            .route_layer(from_fn(inject));

        let req = Request::post("/api/runtime/connections")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── MCP transport "mcp requires api" invariant (SYN-523) ────────────
    //
    // Mirror of the layer stack on `mcp_router` in server.rs: the MCP
    // transport is a *fallback_service*, layered (inner→outer) with
    // `require_mcp`, `require_api`, `api_key_auth_guard`, auth. These tests
    // rebuild that stack with the pure decision functions and pin that
    // `mcp=true, api=false` is unreachable for EVERY auth method — JWT
    // callers hit `require_api`, ApiKey callers hit the bypass guard — and
    // that the default all-features-on snapshot is unaffected.

    fn mcp_stack_app(snapshot: EntitlementSnapshot, method: AuthMethod) -> Router {
        // `.layer()` (not `.route_layer()`) on a fallback, exactly like the
        // real mcp_router — route_layer would skip fallback services.
        Router::new()
            .fallback(post(|| async { dummy_handler() }))
            .layer(from_fn(make_test_gate(snapshot.clone(), FeatureKey::Mcp)))
            .layer(from_fn(make_test_gate(snapshot.clone(), FeatureKey::Api)))
            .layer(from_fn(make_test_api_key_guard(snapshot)))
            .layer(from_fn(inject_auth_context(ctx_with(method))))
    }

    async fn mcp_post(app: Router) -> Response {
        let req = Request::post("/mcp").body(Body::empty()).unwrap();
        app.oneshot(req).await.unwrap()
    }

    #[tokio::test]
    async fn mcp_without_api_denies_jwt_callers_with_feature_api() {
        // Before SYN-523 this was the asymmetric hole: JWT MCP clients
        // passed while ApiKey clients were rejected. Now the `require_api`
        // layer denies JWT callers too, with feature=api.
        let snapshot = snapshot_with(None, Some(r#"{"features":{"mcp":true,"api":false}}"#));
        let resp = mcp_post(mcp_stack_app(snapshot, AuthMethod::Jwt)).await;

        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = body_json(resp).await;
        assert_eq!(body["code"], codes::ENTITLEMENT_REQUIRED);
        assert_eq!(body["feature"], "api");
    }

    #[tokio::test]
    async fn mcp_without_api_denies_api_key_callers_unchanged() {
        // ApiKey callers were already rejected by `api_key_auth_guard`;
        // adding `require_api` must not change the observable outcome.
        let snapshot = snapshot_with(None, Some(r#"{"features":{"mcp":true,"api":false}}"#));
        let resp = mcp_post(mcp_stack_app(snapshot, AuthMethod::ApiKey)).await;

        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = body_json(resp).await;
        assert_eq!(body["code"], codes::ENTITLEMENT_REQUIRED);
        assert_eq!(body["feature"], "api");
    }

    #[tokio::test]
    async fn mcp_default_snapshot_unaffected_by_require_api_layer() {
        // Dark-launch control: default snapshot (all features on) must sail
        // through the extended stack for both auth methods.
        for method in [AuthMethod::Jwt, AuthMethod::ApiKey] {
            let snapshot = snapshot_with(None, None);
            let resp = mcp_post(mcp_stack_app(snapshot, method)).await;
            assert_eq!(resp.status(), StatusCode::OK);
            let bytes = axum::body::to_bytes(resp.into_body(), 1024)
                .await
                .expect("read body");
            assert_eq!(&bytes[..], b"ok");
        }
    }

    #[tokio::test]
    async fn mcp_disabled_still_denies_with_feature_mcp() {
        // api on, mcp off → the inner `require_mcp` gate is the one that
        // fires, so the denial names feature=mcp, not api.
        let snapshot = snapshot_with(None, Some(r#"{"features":{"mcp":false,"api":true}}"#));
        let resp = mcp_post(mcp_stack_app(snapshot, AuthMethod::Jwt)).await;

        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = body_json(resp).await;
        assert_eq!(body["code"], codes::ENTITLEMENT_REQUIRED);
        assert_eq!(body["feature"], "mcp");
    }
}
