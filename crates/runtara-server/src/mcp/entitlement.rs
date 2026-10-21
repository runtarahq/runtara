//! MCP-side entitlement gates.
//!
//! Each MCP tool that surfaces a gated feature or references a specific
//! agent module calls one of these helpers at the top to fail fast with a
//! stable `code` instead of round-tripping through the internal HTTP router
//! only to be rejected at the REST gate.
//!
//! Both helpers return [`rmcp::ErrorData`] built by
//! [`crate::entitlement_error::EntitlementDenial::to_rmcp_error`], so the wire
//! shape that an MCP client sees is identical regardless of whether the gate
//! fired here (tool layer) or via the 403-to-rmcp translation in
//! [`crate::mcp::tools::internal_api`] (REST layer fallback).
//!
//! The resolved snapshot lives in `config::entitlements()` for the whole
//! process. The `server` argument carries the tenant for the per-tenant part:
//! [`require_agent`] exempts the tenant's published workflow-agents.

use rmcp::ErrorData;

use crate::entitlement_error::EntitlementDenial;
use crate::entitlements::FeatureKey;
use crate::mcp::server::SmoMcpServer;

/// Fail with a stable `ENTITLEMENT_REQUIRED` rmcp error when the named
/// feature is disabled for the running tenant.
pub fn require_feature(_server: &SmoMcpServer, feature: FeatureKey) -> Result<(), ErrorData> {
    crate::config::entitlements()
        .require_feature(feature)
        .map_err(EntitlementDenial::from)
        .map_err(|d| d.to_rmcp_error())
}

/// Fail with a stable `AGENT_NOT_ENABLED` rmcp error when the given agent
/// module is not in the tenant's allowlist (or not a registered dispatcher
/// module at all). The tenant's published workflow-agents are its own
/// workflows, not licensed integrations, so they are exempt, as on save;
/// see [`crate::middleware::entitlement::tenant_agent_decision`].
pub fn require_agent(server: &SmoMcpServer, module_id: &str) -> Result<(), ErrorData> {
    crate::middleware::entitlement::tenant_agent_decision(
        crate::config::entitlements(),
        module_id,
        || crate::workflow_agents::published_agent_ids(&server.tenant_id),
    )
    .map_err(|d| d.to_rmcp_error())
}
