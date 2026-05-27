import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router';

// Auth mock — useEntitlements ultimately calls useCustomQuery, which uses
// useAuth for the token. We only care about the entitlement gate here.
vi.mock('react-oidc-context', () => ({
  useAuth: () => ({ user: { access_token: 'test-token' } }),
}));

// Prevent the hook from ever attempting a real fetch — these tests run with
// the inlined snapshot path only.
vi.mock('@/shared/queries/entitlements', () => ({
  fetchEntitlements: vi.fn().mockResolvedValue(undefined),
}));

import { EntitlementRoute } from './EntitlementRoute';
import type { EntitlementsSnapshot } from '@/shared/entitlements';

function withSnapshot(snapshot: EntitlementsSnapshot) {
  window.__RUNTARA_CONFIG__ = { entitlements: snapshot };
}

function snapshot(
  features: Partial<EntitlementsSnapshot['features']>
): EntitlementsSnapshot {
  return {
    tenantId: 'tenant-test',
    pricingTier: 'default',
    features: {
      reports: false,
      database: false,
      api: false,
      mcp: false,
      ...features,
    },
    agents: [],
    limits: {},
  };
}

function renderGuarded(feature: 'reports' | 'database' | 'api' | 'mcp') {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter>
        <EntitlementRoute feature={feature}>
          <div data-testid="real-child">REAL_CHILD</div>
        </EntitlementRoute>
      </MemoryRouter>
    </QueryClientProvider>
  );
}

describe('EntitlementRoute', () => {
  beforeEach(() => {
    delete window.__RUNTARA_CONFIG__;
  });

  afterEach(() => {
    delete window.__RUNTARA_CONFIG__;
  });

  it('renders children when the feature is enabled', () => {
    withSnapshot(snapshot({ reports: true }));
    renderGuarded('reports');
    expect(screen.getByTestId('real-child')).toBeInTheDocument();
    // Disabled-page sentinel should NOT appear.
    expect(screen.queryByText(/is not enabled/i)).not.toBeInTheDocument();
  });

  it('renders FeatureDisabled when the feature is disabled', () => {
    withSnapshot(snapshot({ reports: false }));
    renderGuarded('reports');
    expect(screen.queryByTestId('real-child')).not.toBeInTheDocument();
    // Heading is generic ("Feature not enabled") and the label appears in the
    // body — this avoids subject-verb-agreement issues for plural labels like
    // "Reports".
    expect(
      screen.getByRole('heading', { name: /feature not enabled/i })
    ).toBeInTheDocument();
    expect(screen.getByText(/Reports/)).toBeInTheDocument();
    // Back link is always rendered.
    expect(screen.getByRole('link', { name: /back to workflows/i }))
      .toHaveAttribute('href', '/workflows');
  });

  it('uses the right human-readable label for database', () => {
    withSnapshot(snapshot({ database: false }));
    renderGuarded('database');
    expect(screen.getByText('Database')).toBeInTheDocument();
  });

  it('uses the right human-readable label for api', () => {
    withSnapshot(snapshot({ api: false }));
    renderGuarded('api');
    expect(screen.getByText('API access')).toBeInTheDocument();
  });

  it('uses the right human-readable label for mcp', () => {
    withSnapshot(snapshot({ mcp: false }));
    renderGuarded('mcp');
    expect(screen.getByText('MCP')).toBeInTheDocument();
  });

  it('isolates feature checks — disabling X does not block Y', () => {
    // Snapshot disables reports but enables database. Database route must
    // still render children, proving the gate is feature-scoped.
    withSnapshot(snapshot({ reports: false, database: true }));
    renderGuarded('database');
    expect(screen.getByTestId('real-child')).toBeInTheDocument();
  });
});
