//! Entitlements handler.
//!
//! `GET /api/runtime/entitlements` returns the resolved entitlement snapshot
//! for the process's single tenant. Authentication is required so the route
//! sits behind `tenant_routes`; the snapshot itself is a per-process value
//! built once at startup (see `crate::config::entitlements`).

use axum::response::Json;

use crate::api::dto::entitlements::EntitlementsDto;
use crate::middleware::tenant_auth::OrgId;

#[utoipa::path(
    get,
    path = "/api/runtime/entitlements",
    tag = "entitlements-controller",
    responses(
        (status = 200, description = "Resolved entitlement snapshot for the authenticated tenant", body = EntitlementsDto),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_entitlements_handler(OrgId(_tenant_id): OrgId) -> Json<EntitlementsDto> {
    Json(EntitlementsDto::from(crate::config::entitlements()))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::json;

    use crate::api::dto::entitlements::EntitlementsDto;
    use crate::entitlements::{EntitlementSnapshot, FeatureKey, Tier, parse_agents};

    fn registered() -> BTreeSet<String> {
        parse_agents(&["http", "csv", "xml", "openai", "anthropic"])
    }

    fn snapshot(
        pricing_tier: Option<&str>,
        entitlements_json: Option<&str>,
        overrides_json: Option<&str>,
    ) -> EntitlementSnapshot {
        EntitlementSnapshot::parse_entitlements(
            "tenant-123",
            pricing_tier,
            entitlements_json,
            overrides_json,
            &registered(),
        )
        .expect("snapshot parses")
    }

    #[test]
    fn dto_uses_camel_case_wire_keys() {
        let dto = EntitlementsDto::from(&snapshot(None, None, None));
        let value = serde_json::to_value(&dto).unwrap();
        let obj = value.as_object().expect("object");

        for key in ["tenantId", "pricingTier", "features", "agents", "limits"] {
            assert!(obj.contains_key(key), "missing key {key}: {value}");
        }
        assert!(!obj.contains_key("tenant_id"));
        assert!(!obj.contains_key("pricing_tier"));
    }

    #[test]
    fn dto_tenant_id_is_propagated() {
        let dto = EntitlementsDto::from(&snapshot(None, None, None));
        assert_eq!(dto.tenant_id, "tenant-123");
    }

    #[test]
    fn pricing_tier_serialises_as_lowercase_string() {
        let dto = EntitlementsDto::from(&snapshot(Some("starter"), None, None));
        let value = serde_json::to_value(&dto).unwrap();
        assert_eq!(value["pricingTier"], json!("starter"));
    }

    #[test]
    fn default_tier_serialises_as_default_string() {
        let dto = EntitlementsDto::from(&snapshot(None, None, None));
        let value = serde_json::to_value(&dto).unwrap();
        assert_eq!(value["pricingTier"], json!("default"));
    }

    #[test]
    fn features_serialise_with_snake_case_keys() {
        let dto = EntitlementsDto::from(&snapshot(None, None, None));
        let value = serde_json::to_value(&dto).unwrap();
        let features = value["features"].as_object().expect("features object");
        for key in ["reports", "database", "api", "mcp"] {
            assert_eq!(features[key], json!(true), "expected {key} = true");
        }
    }

    #[test]
    fn features_reflect_disabled_state() {
        let dto = EntitlementsDto::from(&snapshot(
            None,
            Some(r#"{"features":{"reports":false}}"#),
            None,
        ));
        assert!(!dto.features[&FeatureKey::Reports]);
        assert!(dto.features[&FeatureKey::Database]);
    }

    #[test]
    fn agents_are_materialised_from_registered_when_unset() {
        // enabled_agents = None → wire payload lists every registered agent.
        let dto = EntitlementsDto::from(&snapshot(None, None, None));
        let mut expected: Vec<String> = registered().into_iter().collect();
        expected.sort();
        let mut got = dto.agents.clone();
        got.sort();
        assert_eq!(got, expected);
    }

    #[test]
    fn agents_reflect_explicit_allowlist() {
        let dto =
            EntitlementsDto::from(&snapshot(None, Some(r#"{"agents":["http","csv"]}"#), None));
        let mut got = dto.agents.clone();
        got.sort();
        assert_eq!(got, vec!["csv".to_string(), "http".to_string()]);
    }

    #[test]
    fn explicit_empty_agents_serialises_as_empty_array() {
        let dto = EntitlementsDto::from(&snapshot(None, Some(r#"{"agents":[]}"#), None));
        assert!(dto.agents.is_empty());
        let value = serde_json::to_value(&dto).unwrap();
        assert_eq!(value["agents"], json!([]));
    }

    #[test]
    fn limits_round_trip_with_camel_case() {
        let dto = EntitlementsDto::from(&snapshot(
            None,
            Some(r#"{"limits":{"maxApiKeys":5,"maxWorkflows":10}}"#),
            None,
        ));
        let value = serde_json::to_value(&dto).unwrap();
        assert_eq!(value["limits"]["maxApiKeys"], json!(5));
        assert_eq!(value["limits"]["maxWorkflows"], json!(10));
    }

    #[test]
    fn full_payload_matches_doc_shape_for_premium_tier() {
        // Spot-check the full shape for a non-default tier so that we catch
        // accidental key renames in a single assertion.
        let dto = EntitlementsDto::from(&snapshot(Some("premium"), None, None));
        let value = serde_json::to_value(&dto).unwrap();

        assert_eq!(value["tenantId"], json!("tenant-123"));
        assert_eq!(value["pricingTier"], json!("premium"));
        assert_eq!(value["features"]["reports"], json!(true));
        assert_eq!(value["features"]["database"], json!(true));
        assert_eq!(value["features"]["api"], json!(true));
        assert_eq!(value["features"]["mcp"], json!(false));
        assert_eq!(value["limits"]["maxApiKeys"], json!(10));
        assert!(value["agents"].is_array());
    }

    #[test]
    fn dto_pricing_tier_field_matches_snapshot() {
        let dto = EntitlementsDto::from(&snapshot(Some("enterprise"), None, None));
        assert_eq!(dto.pricing_tier, Tier::Enterprise);
    }
}
