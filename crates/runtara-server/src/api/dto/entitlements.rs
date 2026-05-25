//! Entitlements DTO.
//!
//! Wire shape returned by `GET /api/runtime/entitlements` and inlined into
//! `window.__RUNTARA_CONFIG__.entitlements` by the UI handler.
//!
//! Mirrors the contract in `docs/entitlements.md`:
//! - camelCase keys (`tenantId`, `pricingTier`),
//! - `pricingTier` as a lowercase string (`default`, `starter`, `premium`, `enterprise`),
//! - `agents` as a concrete array — the internal "`enabled_agents = None`
//!   means all registered agents" sentinel is materialised here so the frontend
//!   never has to reason about an implicit-all.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use utoipa::ToSchema;

use crate::entitlements::{EntitlementLimits, EntitlementSnapshot, FeatureKey, Tier};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EntitlementsDto {
    pub tenant_id: String,
    pub pricing_tier: Tier,
    pub features: BTreeMap<FeatureKey, bool>,
    pub agents: Vec<String>,
    pub limits: EntitlementLimits,
}

impl From<&EntitlementSnapshot> for EntitlementsDto {
    fn from(snapshot: &EntitlementSnapshot) -> Self {
        EntitlementsDto {
            tenant_id: snapshot.tenant_id.clone(),
            pricing_tier: snapshot.pricing_tier.clone(),
            features: snapshot.features().clone(),
            agents: snapshot.materialised_agents().into_iter().collect(),
            limits: snapshot.limits.clone(),
        }
    }
}
