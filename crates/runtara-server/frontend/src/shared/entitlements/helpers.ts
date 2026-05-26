import type { EntitlementsSnapshot, FeatureKey } from './types';

/**
 * Permissive fallback used when neither `window.__RUNTARA_CONFIG__.entitlements`
 * nor `GET /api/runtime/entitlements` is available — see Phase 4.2 in
 * `docs/entitlements.md`. Matches the backend's "no entitlement env set"
 * default so the UI mirrors what an unconfigured server would expose.
 *
 * In normal operation (server-rendered HTML), the inlined snapshot is always
 * present and this fallback is never reached. It exists for `vite dev`,
 * Storybook, and vitest contexts where the Rust UI handler isn't in the loop.
 */
export const PERMISSIVE_FALLBACK: EntitlementsSnapshot = {
  tenantId: 'unknown',
  pricingTier: 'default',
  features: {
    reports: true,
    database: true,
    api: true,
    mcp: true,
  },
  agents: [],
  limits: {},
};

/**
 * True when `feature` is explicitly enabled in the snapshot. An absent key is
 * treated as disabled — the backend resolves every `FeatureKey` at startup, so
 * a missing key indicates a contract drift between client and server, and
 * defaulting to "off" matches the backend's enforcement default.
 */
export function isEnabled(
  snapshot: EntitlementsSnapshot,
  feature: FeatureKey
): boolean {
  return snapshot.features[feature] === true;
}

/**
 * True when `moduleId` appears in the snapshot's materialised agent allowlist.
 * The backend always serialises a concrete array (`None` is resolved against
 * the registered modules before serialising), so set-membership is the right
 * check — empty list means "no agents enabled".
 */
export function agentEnabled(
  snapshot: EntitlementsSnapshot,
  moduleId: string
): boolean {
  return snapshot.agents.includes(moduleId);
}
