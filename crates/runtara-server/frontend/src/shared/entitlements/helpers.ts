import type { EntitlementsSnapshot, FeatureKey } from './types';

/**
 * Human-readable label for each feature key. Used wherever a feature is shown
 * to the user — disabled-state page, 403 toast titles, etc. Mirrors
 * `FeatureKey::display_name` in `crates/runtara-server/src/entitlements.rs`.
 */
export const FEATURE_LABELS: Record<FeatureKey, string> = {
  reports: 'Reports',
  database: 'Database',
  api: 'API access',
  mcp: 'MCP',
};

/** True when `value` is a valid `FeatureKey` string from the wire. Use to
 *  narrow `string | undefined` from error bodies before indexing FEATURE_LABELS. */
export function isFeatureKey(value: unknown): value is FeatureKey {
  return (
    typeof value === 'string' &&
    Object.prototype.hasOwnProperty.call(FEATURE_LABELS, value)
  );
}

/**
 * Permissive fallback used when neither `window.__RUNTARA_CONFIG__.entitlements`
 * nor `GET /api/runtime/entitlements` is available. Matches the backend's
 * "no entitlement env set" default so the UI mirrors what an unconfigured
 * server would expose — see `docs/entitlements.md`.
 *
 * In normal operation (server-rendered HTML), the inlined snapshot is always
 * present and this fallback is never reached. It exists for `vite dev`,
 * Storybook, and vitest contexts where the Rust UI handler isn't in the loop.
 *
 * `agents: []` here is intentional and *does not* mean "no agents allowed"
 * for the fallback. The wire shape always sends a concrete array (backend
 * pre-materialises `None → all known modules`), but the frontend has no
 * registry to materialise against when the backend is unreachable. The
 * empty array is paired with an identity-based escape hatch in
 * `agentEnabled()` — see below. Treat this constant as a frozen singleton:
 * do not mutate, copy, or pass through structural cloning, or the identity
 * check is lost and the fallback becomes "deny all agents".
 */
export const PERMISSIVE_FALLBACK: EntitlementsSnapshot = Object.freeze({
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
}) as EntitlementsSnapshot;

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
 * check for any real snapshot — empty list means "no agents enabled".
 *
 * Special case: the frontend `PERMISSIVE_FALLBACK` is the *only* snapshot in
 * the SPA that legitimately carries an empty `agents` array while meaning
 * "everything allowed" — see the comment on PERMISSIVE_FALLBACK above for why.
 * Without this short-circuit, agent-bearing surfaces (Step Picker, stale-agent
 * badge, etc.) would collapse to "deny all" in `vite dev` / Storybook / test
 * contexts where the fallback is the only thing available — directly
 * contradicting the fallback's "everything allowed" intent.
 */
export function agentEnabled(
  snapshot: EntitlementsSnapshot,
  moduleId: string
): boolean {
  if (snapshot === PERMISSIVE_FALLBACK) return true;
  return snapshot.agents.includes(moduleId);
}

/**
 * Build an allowlist set suitable for passing to `getAgents()` as the
 * `enabledAgentIds` filter. Returns `undefined` for the permissive fallback
 * — `getAgents()` treats `undefined` as "no filter", i.e. accept every
 * registered module. Any other snapshot collapses to a concrete
 * `ReadonlySet<string>` of the explicit allowlist.
 *
 * This is the structural counterpart to `agentEnabled`: anywhere code
 * *builds* an allowlist (rather than asking "is X allowed?") must go
 * through this helper so the permissive-fallback contract holds. Without
 * it, callers that do `new Set(snap.agents)` would collapse the fallback's
 * empty array into a deny-all set, flipping the documented semantics.
 */
export function enabledAgentSet(
  snapshot: EntitlementsSnapshot
): ReadonlySet<string> | undefined {
  if (snapshot === PERMISSIVE_FALLBACK) return undefined;
  return new Set(snapshot.agents);
}
