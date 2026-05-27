import { describe, expect, it } from 'vitest';
import {
  agentEnabled,
  enabledAgentSet,
  isEnabled,
  PERMISSIVE_FALLBACK,
} from './helpers';
import type { EntitlementsSnapshot } from './types';

function snapshot(
  overrides: Partial<EntitlementsSnapshot> = {}
): EntitlementsSnapshot {
  return {
    tenantId: 'tenant-test',
    pricingTier: 'default',
    features: { reports: true, database: true, api: true, mcp: true },
    agents: ['http', 'csv', 'openai'],
    limits: {},
    ...overrides,
  };
}

describe('isEnabled', () => {
  it('returns true when the feature is explicitly enabled', () => {
    const snap = snapshot();
    expect(isEnabled(snap, 'reports')).toBe(true);
    expect(isEnabled(snap, 'database')).toBe(true);
    expect(isEnabled(snap, 'api')).toBe(true);
    expect(isEnabled(snap, 'mcp')).toBe(true);
  });

  it('returns false when the feature is explicitly disabled', () => {
    const snap = snapshot({
      features: { reports: false, database: true, api: true, mcp: true },
    });
    expect(isEnabled(snap, 'reports')).toBe(false);
    expect(isEnabled(snap, 'database')).toBe(true);
  });

  it('returns false when the feature key is absent (defaults to off)', () => {
    // Mirrors backend default: an unresolvable feature is treated as denied.
    const snap = snapshot({ features: {} });
    expect(isEnabled(snap, 'reports')).toBe(false);
  });

  it('treats non-true values as disabled (no truthy coercion)', () => {
    // The wire shape is boolean-valued; defensive guard so a future contract
    // change can't sneak in `1` or `"yes"` and flip enforcement silently.
    const snap = snapshot({
      // @ts-expect-error — deliberately wrong type to exercise the guard.
      features: { reports: 1, database: 'true' },
    });
    expect(isEnabled(snap, 'reports')).toBe(false);
    expect(isEnabled(snap, 'database')).toBe(false);
  });
});

describe('agentEnabled', () => {
  it('returns true when the module is in the allowlist', () => {
    const snap = snapshot();
    expect(agentEnabled(snap, 'http')).toBe(true);
    expect(agentEnabled(snap, 'openai')).toBe(true);
  });

  it('returns false when the module is not in the allowlist', () => {
    const snap = snapshot();
    expect(agentEnabled(snap, 'anthropic')).toBe(false);
  });

  it('returns false for any agent when the allowlist is empty', () => {
    const snap = snapshot({ agents: [] });
    expect(agentEnabled(snap, 'http')).toBe(false);
    expect(agentEnabled(snap, 'openai')).toBe(false);
  });
});

describe('PERMISSIVE_FALLBACK', () => {
  it('enables every feature key the SPA branches on', () => {
    // Mirrors backend "no entitlement env set" default — see
    // `docs/entitlements.md#local-development-default`.
    expect(isEnabled(PERMISSIVE_FALLBACK, 'reports')).toBe(true);
    expect(isEnabled(PERMISSIVE_FALLBACK, 'database')).toBe(true);
    expect(isEnabled(PERMISSIVE_FALLBACK, 'api')).toBe(true);
    expect(isEnabled(PERMISSIVE_FALLBACK, 'mcp')).toBe(true);
  });

  it('treats every agent as enabled despite the empty agents array', () => {
    // Regression guard. Without the identity-based short-circuit in
    // `agentEnabled`, the fallback's empty `agents` array would deny every
    // agent — flipping the documented "permissive" contract on its head and
    // collapsing the Step Picker in `vite dev`, Storybook, and tests that
    // don't provide a snapshot.
    expect(agentEnabled(PERMISSIVE_FALLBACK, 'http')).toBe(true);
    expect(agentEnabled(PERMISSIVE_FALLBACK, 'openai')).toBe(true);
    expect(agentEnabled(PERMISSIVE_FALLBACK, 'anything-at-all')).toBe(true);
  });

  it('does not extend permissive semantics to *copies* of the fallback', () => {
    // The short-circuit is identity-based on purpose: a real snapshot that
    // happens to have an empty agents array (explicit "deny all" allowlist)
    // must keep denying every agent. Only the singleton constant exported
    // from this module gets the permissive treatment.
    const copy = { ...PERMISSIVE_FALLBACK };
    expect(agentEnabled(copy, 'http')).toBe(false);
  });
});

describe('enabledAgentSet', () => {
  it('returns undefined for the PERMISSIVE_FALLBACK so callers do not filter', () => {
    // The whole point of this helper: callers like `getAgents(token, set)`
    // treat `undefined` as "no filter, accept everything". Without it, the
    // fallback's empty `agents` array would collapse the agent registry to
    // an empty list in `vite dev` / Storybook / failed-fetch contexts.
    expect(enabledAgentSet(PERMISSIVE_FALLBACK)).toBeUndefined();
  });

  it('returns a concrete Set for any real snapshot', () => {
    const snap = snapshot({ agents: ['http', 'csv'] });
    const set = enabledAgentSet(snap);
    expect(set).toBeInstanceOf(Set);
    expect(set?.has('http')).toBe(true);
    expect(set?.has('csv')).toBe(true);
    expect(set?.has('openai')).toBe(false);
  });

  it('preserves explicit deny-all semantics for a real empty allowlist', () => {
    // A real snapshot with `agents: []` is the explicit "deny everything"
    // case — must NOT be treated like the permissive fallback. The
    // identity-based short-circuit means copies of the fallback do not
    // count; only the singleton triggers the undefined return.
    const denyAll: EntitlementsSnapshot = { ...PERMISSIVE_FALLBACK, agents: [] };
    const set = enabledAgentSet(denyAll);
    expect(set).toBeInstanceOf(Set);
    expect(set?.size).toBe(0);
  });
});
