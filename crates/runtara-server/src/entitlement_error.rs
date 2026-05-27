//! Shared entitlement denial responses.
//!
//! Every place where an entitlement gate denies a request goes through this
//! module, so the stable error `code`, status, and JSON body shape are
//! defined in exactly one spot — see `docs/entitlements.md`. REST handlers
//! return `EntitlementDenial` (it implements `IntoResponse`); MCP tools call
//! [`EntitlementDenial::to_rmcp_error`] to surface the same structured
//! information through the JSON-RPC envelope with `code` preserved in `data`.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::entitlements::{EntitlementError, FeatureKey};

/// Stable error codes returned in the response body. The UI, MCP clients, and
/// downstream tooling switch on these strings, so they MUST NOT change.
/// See `docs/entitlements.md:200-237`.
pub mod codes {
    pub const ENTITLEMENT_REQUIRED: &str = "ENTITLEMENT_REQUIRED";
    pub const AGENT_NOT_ENABLED: &str = "AGENT_NOT_ENABLED";
    pub const ENTITLEMENT_LIMIT_EXCEEDED: &str = "ENTITLEMENT_LIMIT_EXCEEDED";
}

/// The three documented entitlement denials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntitlementDenial {
    /// A feature gate ran and the feature is disabled for this tenant.
    /// Wire `code`: `ENTITLEMENT_REQUIRED`.
    FeatureRequired(FeatureKey),

    /// A specific agent module is not in the tenant's allowlist (or not
    /// registered at all). Wire `code`: `AGENT_NOT_ENABLED`.
    AgentNotEnabled(String),

    /// A numeric tier limit would be exceeded by this request.
    /// Wire `code`: `ENTITLEMENT_LIMIT_EXCEEDED`.
    LimitExceeded {
        /// Stable camelCase limit identifier matching the entitlement snapshot
        /// field name (e.g. `"maxApiKeys"`).
        limit: &'static str,
        /// The configured maximum the caller would breach.
        maximum: u64,
    },
}

impl EntitlementDenial {
    /// Stable wire `code` string.
    pub const fn code(&self) -> &'static str {
        match self {
            EntitlementDenial::FeatureRequired(_) => codes::ENTITLEMENT_REQUIRED,
            EntitlementDenial::AgentNotEnabled(_) => codes::AGENT_NOT_ENABLED,
            EntitlementDenial::LimitExceeded { .. } => codes::ENTITLEMENT_LIMIT_EXCEEDED,
        }
    }

    /// Short, human-readable summary that goes in the body `error` field.
    pub const fn error_summary(&self) -> &'static str {
        match self {
            EntitlementDenial::FeatureRequired(_) => "Entitlement required",
            EntitlementDenial::AgentNotEnabled(_) => "Agent not enabled",
            EntitlementDenial::LimitExceeded { .. } => "Tier limit exceeded",
        }
    }

    /// Default human-readable message. Callers that want a more specific
    /// message can build the JSON body themselves; the code is the stable
    /// contract, not the message.
    pub fn message(&self) -> String {
        match self {
            EntitlementDenial::FeatureRequired(feature) => {
                format!("{} is not enabled for this tenant.", feature.display_name())
            }
            EntitlementDenial::AgentNotEnabled(agent) => {
                format!("Agent '{agent}' is not enabled for this tenant.")
            }
            EntitlementDenial::LimitExceeded { limit, maximum } => {
                format!("Tenant has reached the {limit} limit (maximum {maximum}).")
            }
        }
    }

    /// Build the JSON body. Always includes `error`, `code`, and `message`;
    /// adds `feature` / `agent` / (`limit` + `maximum`) per variant.
    pub fn json_body(&self) -> Value {
        match self {
            EntitlementDenial::FeatureRequired(feature) => json!({
                "error": self.error_summary(),
                "code": self.code(),
                "feature": feature.name(),
                "message": self.message(),
            }),
            EntitlementDenial::AgentNotEnabled(agent) => json!({
                "error": self.error_summary(),
                "code": self.code(),
                "agent": agent,
                "message": self.message(),
            }),
            EntitlementDenial::LimitExceeded { limit, maximum } => json!({
                "error": self.error_summary(),
                "code": self.code(),
                "limit": limit,
                "maximum": maximum,
                "message": self.message(),
            }),
        }
    }

    /// MCP-side variant. JSON-RPC's `code` field is reserved for the standard
    /// JSON-RPC codes, so we use `INVALID_REQUEST` there and carry the full
    /// HTTP-shaped body (including the stable `code` string) in `data`. MCP
    /// clients switch on `data.code`, not on the JSON-RPC code.
    pub fn to_rmcp_error(&self) -> rmcp::ErrorData {
        self.audit_log(crate::config::try_tenant_id().unwrap_or("<unset>"));
        rmcp::ErrorData::new(
            rmcp::model::ErrorCode::INVALID_REQUEST,
            self.message(),
            Some(self.json_body()),
        )
    }

    /// Build the operator-facing audit fields for this denial. Pure function
    /// so unit tests can pin down the exact field values without spinning up
    /// `tracing`. Mirrors the shape `audit_log` hands to `tracing::warn!`.
    pub fn audit_fields<'a>(&'a self, tenant_id: &'a str) -> DenialAuditFields<'a> {
        let mut fields = DenialAuditFields {
            code: self.code(),
            tenant_id,
            feature: None,
            agent: None,
            limit: None,
            maximum: None,
        };
        match self {
            EntitlementDenial::FeatureRequired(feature) => {
                fields.feature = Some(feature.name());
            }
            EntitlementDenial::AgentNotEnabled(agent) => {
                fields.agent = Some(agent.as_str());
            }
            EntitlementDenial::LimitExceeded { limit, maximum } => {
                fields.limit = Some(*limit);
                fields.maximum = Some(*maximum);
            }
        }
        fields
    }

    /// Emit one structured `tracing::warn!` line describing this denial.
    /// Called from both [`IntoResponse`] and [`Self::to_rmcp_error`] so every
    /// entitlement denial in the process produces exactly one log entry.
    ///
    /// Per-variant fields (`feature` / `agent` / `limit` + `maximum`) are
    /// emitted as `Option`s — only the relevant one is `Some` for any given
    /// denial. Operators can grep by `code` for any denial class, or by
    /// the variant-specific field for a more targeted slice.
    pub fn audit_log(&self, tenant_id: &str) {
        let f = self.audit_fields(tenant_id);
        tracing::warn!(
            code = f.code,
            tenant_id = f.tenant_id,
            feature = ?f.feature,
            agent = ?f.agent,
            limit = ?f.limit,
            maximum = ?f.maximum,
            "entitlement denial"
        );
    }
}

/// Plain-data shape of the audit-log fields for an entitlement denial.
/// Returned by [`EntitlementDenial::audit_fields`] and consumed by
/// [`EntitlementDenial::audit_log`]. Per-variant fields are `Option` because
/// only one applies at a time; `code` and `tenant_id` are always populated.
#[derive(Debug, Clone, PartialEq)]
pub struct DenialAuditFields<'a> {
    pub code: &'static str,
    pub tenant_id: &'a str,
    /// `Some(snake_case key)` only for `FeatureRequired`.
    pub feature: Option<&'static str>,
    /// `Some(module id)` only for `AgentNotEnabled`.
    pub agent: Option<&'a str>,
    /// `Some(camelCase limit name)` only for `LimitExceeded`.
    pub limit: Option<&'static str>,
    /// `Some(numeric cap)` only for `LimitExceeded`.
    pub maximum: Option<u64>,
}

impl IntoResponse for EntitlementDenial {
    fn into_response(self) -> Response {
        self.audit_log(crate::config::try_tenant_id().unwrap_or("<unset>"));
        (StatusCode::FORBIDDEN, Json(self.json_body())).into_response()
    }
}

/// Bridge `EntitlementSnapshot::require_*` results into the wire denial type
/// so handlers can `?` the snapshot calls directly.
impl From<EntitlementError> for EntitlementDenial {
    fn from(err: EntitlementError) -> Self {
        match err {
            EntitlementError::FeatureDisabled(feature) => {
                EntitlementDenial::FeatureRequired(feature)
            }
            EntitlementError::AgentNotEnabled(agent) => EntitlementDenial::AgentNotEnabled(agent),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::to_bytes;
    use axum::http::StatusCode;

    fn body_of(value: &Value) -> &serde_json::Map<String, Value> {
        value.as_object().expect("json object body")
    }

    // ── code strings ────────────────────────────────────────────────────

    #[test]
    fn feature_required_code_is_stable() {
        let d = EntitlementDenial::FeatureRequired(FeatureKey::Reports);
        assert_eq!(d.code(), "ENTITLEMENT_REQUIRED");
    }

    #[test]
    fn agent_not_enabled_code_is_stable() {
        let d = EntitlementDenial::AgentNotEnabled("openai".into());
        assert_eq!(d.code(), "AGENT_NOT_ENABLED");
    }

    #[test]
    fn limit_exceeded_code_is_stable() {
        let d = EntitlementDenial::LimitExceeded {
            limit: "maxApiKeys",
            maximum: 10,
        };
        assert_eq!(d.code(), "ENTITLEMENT_LIMIT_EXCEEDED");
    }

    // ── JSON body shape ─────────────────────────────────────────────────

    #[test]
    fn feature_required_body_matches_doc_shape() {
        let body = EntitlementDenial::FeatureRequired(FeatureKey::Reports).json_body();
        let obj = body_of(&body);

        assert_eq!(obj["error"], json!("Entitlement required"));
        assert_eq!(obj["code"], json!("ENTITLEMENT_REQUIRED"));
        assert_eq!(obj["feature"], json!("reports"));
        assert_eq!(
            obj["message"],
            json!("Reports is not enabled for this tenant.")
        );
        assert!(!obj.contains_key("agent"));
        assert!(!obj.contains_key("limit"));
    }

    #[test]
    fn agent_not_enabled_body_matches_doc_shape() {
        let body = EntitlementDenial::AgentNotEnabled("openai".into()).json_body();
        let obj = body_of(&body);

        assert_eq!(obj["error"], json!("Agent not enabled"));
        assert_eq!(obj["code"], json!("AGENT_NOT_ENABLED"));
        assert_eq!(obj["agent"], json!("openai"));
        assert_eq!(
            obj["message"],
            json!("Agent 'openai' is not enabled for this tenant.")
        );
        assert!(!obj.contains_key("feature"));
        assert!(!obj.contains_key("limit"));
    }

    #[test]
    fn limit_exceeded_body_matches_doc_shape() {
        let body = EntitlementDenial::LimitExceeded {
            limit: "maxApiKeys",
            maximum: 10,
        }
        .json_body();
        let obj = body_of(&body);

        assert_eq!(obj["error"], json!("Tier limit exceeded"));
        assert_eq!(obj["code"], json!("ENTITLEMENT_LIMIT_EXCEEDED"));
        assert_eq!(obj["limit"], json!("maxApiKeys"));
        assert_eq!(obj["maximum"], json!(10));
        assert!(obj["message"].as_str().unwrap().contains("maxApiKeys"));
        assert!(!obj.contains_key("feature"));
        assert!(!obj.contains_key("agent"));
    }

    // ── HTTP variant ────────────────────────────────────────────────────

    #[tokio::test]
    async fn http_response_returns_403_with_json_body() {
        let denial = EntitlementDenial::FeatureRequired(FeatureKey::Mcp);
        let expected = denial.json_body();
        let response = denial.into_response();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            response
                .headers()
                .get("content-type")
                .map(|v| v.to_str().unwrap()),
            Some("application/json")
        );

        let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
        let got: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(got, expected);
    }

    // ── MCP variant ─────────────────────────────────────────────────────

    #[test]
    fn rmcp_error_preserves_stable_code_in_data() {
        let denial = EntitlementDenial::AgentNotEnabled("openai".into());
        let err = denial.to_rmcp_error();

        // JSON-RPC code (the protocol-level field) is the standard one.
        assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_REQUEST);
        // Stable application-level code travels in `data`.
        let data = err.data.expect("data populated");
        assert_eq!(data["code"], json!("AGENT_NOT_ENABLED"));
        assert_eq!(data["agent"], json!("openai"));
        assert_eq!(err.message.as_ref(), denial.message().as_str());
    }

    #[test]
    fn rmcp_error_data_matches_http_body() {
        let denial = EntitlementDenial::LimitExceeded {
            limit: "maxWorkflows",
            maximum: 100,
        };
        let err = denial.to_rmcp_error();
        assert_eq!(err.data.unwrap(), denial.json_body());
    }

    // ── Bridge from snapshot::require_* ─────────────────────────────────

    #[test]
    fn from_entitlement_error_feature_maps_correctly() {
        let denial: EntitlementDenial =
            EntitlementError::FeatureDisabled(FeatureKey::Database).into();
        assert_eq!(
            denial,
            EntitlementDenial::FeatureRequired(FeatureKey::Database)
        );
    }

    #[test]
    fn from_entitlement_error_agent_maps_correctly() {
        let denial: EntitlementDenial = EntitlementError::AgentNotEnabled("xml".into()).into();
        assert_eq!(denial, EntitlementDenial::AgentNotEnabled("xml".into()));
    }

    // ── Feature-key wire helpers (sanity) ───────────────────────────────

    #[test]
    fn feature_name_strings_match_serde_rename() {
        // If FeatureKey::name() ever drifts from the serde rename, the
        // features map keys in the snapshot DTO and the `feature` field in
        // error bodies will diverge — catch that here.
        for key in FeatureKey::ALL {
            let from_serde = serde_json::to_value(key).unwrap();
            assert_eq!(from_serde.as_str().unwrap(), key.name());
        }
    }

    // ── audit_fields() — operator-facing denial log shape ──────────────

    #[test]
    fn audit_fields_feature_required() {
        let d = EntitlementDenial::FeatureRequired(FeatureKey::Reports);
        let f = d.audit_fields("tenant-xyz");
        assert_eq!(f.code, "ENTITLEMENT_REQUIRED");
        assert_eq!(f.tenant_id, "tenant-xyz");
        assert_eq!(f.feature, Some("reports"));
        // Variant-specific fields for the other denial kinds stay None so
        // operators don't grep through stale data.
        assert_eq!(f.agent, None);
        assert_eq!(f.limit, None);
        assert_eq!(f.maximum, None);
    }

    #[test]
    fn audit_fields_agent_not_enabled() {
        let d = EntitlementDenial::AgentNotEnabled("openai".into());
        let f = d.audit_fields("tenant-xyz");
        assert_eq!(f.code, "AGENT_NOT_ENABLED");
        assert_eq!(f.tenant_id, "tenant-xyz");
        assert_eq!(f.agent, Some("openai"));
        assert_eq!(f.feature, None);
        assert_eq!(f.limit, None);
        assert_eq!(f.maximum, None);
    }

    #[test]
    fn audit_fields_limit_exceeded() {
        let d = EntitlementDenial::LimitExceeded {
            limit: "maxApiKeys",
            maximum: 10,
        };
        let f = d.audit_fields("tenant-xyz");
        assert_eq!(f.code, "ENTITLEMENT_LIMIT_EXCEEDED");
        assert_eq!(f.tenant_id, "tenant-xyz");
        assert_eq!(f.limit, Some("maxApiKeys"));
        assert_eq!(f.maximum, Some(10));
        assert_eq!(f.feature, None);
        assert_eq!(f.agent, None);
    }

    #[test]
    fn audit_fields_code_matches_body_code() {
        // The `code` in the audit log MUST match the `code` in the wire
        // body — otherwise operators correlating a 403 a tenant reports
        // with server logs would find divergent codes and waste time.
        for d in [
            EntitlementDenial::FeatureRequired(FeatureKey::Database),
            EntitlementDenial::AgentNotEnabled("xml".into()),
            EntitlementDenial::LimitExceeded {
                limit: "maxWorkflows",
                maximum: 100,
            },
        ] {
            let body_code = d.json_body()["code"].as_str().unwrap().to_string();
            let fields = d.audit_fields("t");
            assert_eq!(fields.code, body_code);
        }
    }
}
