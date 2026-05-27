import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import React from 'react';

// Mock react-oidc-context so useCustomQuery's token wiring is satisfied without
// pulling in an OIDC provider. The hook's `enabled` flag is the gate we care
// about, not auth.
vi.mock('react-oidc-context', () => ({
  useAuth: () => ({ user: { access_token: 'test-token' } }),
}));

// Mock fetchEntitlements at the module boundary so we don't need to spin up
// the real axios client or hit a network. The hook imports it from this path.
vi.mock('@/shared/queries/entitlements', () => ({
  fetchEntitlements: vi.fn(),
}));

import { fetchEntitlements } from '@/shared/queries/entitlements';
import { PERMISSIVE_FALLBACK } from '@/shared/entitlements';
import type { EntitlementsSnapshot } from '@/shared/entitlements';
import { useEntitlements } from './useEntitlements';

const mockFetch = vi.mocked(fetchEntitlements);

function makeSnapshot(
  overrides: Partial<EntitlementsSnapshot> = {}
): EntitlementsSnapshot {
  return {
    tenantId: 'tenant-test',
    pricingTier: 'default',
    features: { reports: true, database: true, api: true, mcp: true },
    agents: ['http', 'csv'],
    limits: {},
    ...overrides,
  };
}

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false, gcTime: 0 },
    },
  });
  return function Wrapper({ children }: { children: React.ReactNode }) {
    return React.createElement(
      QueryClientProvider,
      { client: queryClient },
      children
    );
  };
}

describe('useEntitlements', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // Each test sets its own window.__RUNTARA_CONFIG__ — start from a clean
    // slate so an inlined value from a previous test can't leak across.
    delete window.__RUNTARA_CONFIG__;
  });

  afterEach(() => {
    delete window.__RUNTARA_CONFIG__;
  });

  it('returns the inlined snapshot synchronously and never fetches', async () => {
    const inlined = makeSnapshot({
      tenantId: 'tenant-inlined',
      features: { reports: false, database: true, api: true, mcp: true },
    });
    window.__RUNTARA_CONFIG__ = { entitlements: inlined };

    const { result } = renderHook(() => useEntitlements(), {
      wrapper: wrapper(),
    });

    // The hook should resolve to the inlined value on the very first render —
    // no waitFor, no network round-trip.
    expect(result.current).toEqual(inlined);
    expect(mockFetch).not.toHaveBeenCalled();
  });

  it('falls back to GET /api/runtime/entitlements when nothing is inlined', async () => {
    const fetched = makeSnapshot({
      tenantId: 'tenant-fetched',
      features: { reports: true, database: false, api: true, mcp: false },
    });
    mockFetch.mockResolvedValueOnce(fetched);

    const { result } = renderHook(() => useEntitlements(), {
      wrapper: wrapper(),
    });

    // Initial render: nothing inlined, fetch is in flight — return permissive.
    expect(result.current).toEqual(PERMISSIVE_FALLBACK);

    await waitFor(() => {
      expect(result.current).toEqual(fetched);
    });
    expect(mockFetch).toHaveBeenCalledTimes(1);
  });

  it('falls back to PERMISSIVE_FALLBACK when both inlined and fetched are absent', async () => {
    mockFetch.mockRejectedValueOnce(new Error('network down'));

    const { result } = renderHook(() => useEntitlements(), {
      wrapper: wrapper(),
    });

    // The query starts and fails; the hook should stay on permissive.
    await waitFor(() => {
      expect(mockFetch).toHaveBeenCalledTimes(1);
    });
    expect(result.current).toEqual(PERMISSIVE_FALLBACK);
  });
});
