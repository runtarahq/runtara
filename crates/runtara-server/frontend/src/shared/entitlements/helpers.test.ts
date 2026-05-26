import { describe, expect, it } from 'vitest';
import {
  agentEnabled,
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
});
