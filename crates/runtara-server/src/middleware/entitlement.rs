//! REST entitlement gates.
//!
//! Phase 3.2 of `docs/entitlements.md`. Each `require_*` function is an
//! Axum middleware that checks the process's resolved entitlement snapshot
//! and short-circuits with a 403 + stable `code` (built by
//! [`crate::entitlement_error::EntitlementDenial`]) when the feature is
//! disabled.
//!
//! Gates are mounted in `server.rs` on the smallest sub-router that exactly
//! contains the routes that surface the feature, so the gate is visible at
//! the mount point and cannot leak to neighboring routes.
//!
//! Out of scope for 3.2:
//! - API-key auth bypass on non-management routes (3.3).
//! - Per-agent allowlist (3.4).
//! - MCP tool-level checks beyond the transport gate (3.5).

use axum::{extract::Request, middleware::Next, response::IntoResponse, response::Response};

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

#[cfg(test)]
mod tests {
    //! These tests exercise the pure [`gate_decision`] function against
    //! constructed snapshots. The Axum glue (`require_*`) is a one-line
    //! delegate; its end-to-end HTTP behavior depends on the global
    //! `OnceLock<Config>` and is better covered by a future server-fixture
    //! integration test (Phase 3.2 follow-up).

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
}
