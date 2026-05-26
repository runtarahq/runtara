import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { NodeFormContext } from './NodeFormContext';
import type { ExtendedAgent } from '@/features/workflows/queries';
import type { EntitlementsSnapshot } from '@/shared/entitlements';

// Auth — the hook eventually pulls a token even if no fetch runs.
vi.mock('react-oidc-context', () => ({
  useAuth: () => ({ user: { access_token: 'test-token' } }),
}));

// Avoid hitting any network through useMultipleAgentDetails — return an empty
// map so the picker renders in "no details yet" state. The filtering we care
// about runs before details are needed.
vi.mock('@/features/workflows/hooks', async () => {
  const actual = await vi.importActual<
    typeof import('@/features/workflows/hooks')
  >('@/features/workflows/hooks');
  return {
    ...actual,
    useMultipleAgentDetails: () => ({
      agentDetailsMap: new Map(),
      allLoaded: false,
      isLoading: false,
    }),
  };
});

// useEntitlements is exercised indirectly via the inlined snapshot.
import { StepPickerPanel } from './StepPickerModal';

function snapshot(agents: string[]): EntitlementsSnapshot {
  return {
    tenantId: 'tenant-test',
    pricingTier: 'default',
    features: { reports: true, database: true, api: true, mcp: true },
    agents,
    limits: {},
  };
}

const fakeAgents: ExtendedAgent[] = [
  {
    id: 'http',
    name: 'HTTP',
    description: 'Make HTTP requests',
    supportsConnections: true,
    integrationIds: [],
    supportedCapabilities: {},
  },
  {
    id: 'openai',
    name: 'OpenAI',
    description: 'Chat completions via OpenAI',
    supportsConnections: false,
    integrationIds: [],
    supportedCapabilities: {},
  },
  {
    id: 'csv',
    name: 'CSV',
    description: 'Parse and emit CSV',
    supportsConnections: false,
    integrationIds: [],
    supportedCapabilities: {},
  },
];

function renderPicker(
  snap: EntitlementsSnapshot,
  agentList: ExtendedAgent[] = fakeAgents
) {
  window.__RUNTARA_CONFIG__ = { entitlements: snap };
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter>
        <NodeFormContext.Provider
          value={{
            stepTypes: [],
            agents: agentList,
            workflows: [],
            executionGraph: null,
            isLoading: false,
            previousSteps: [],
          }}
        >
          <StepPickerPanel onSelect={() => {}} />
        </NodeFormContext.Provider>
      </MemoryRouter>
    </QueryClientProvider>
  );
}

describe('StepPickerPanel — entitlement-aware agent filter (Phase 4.6)', () => {
  beforeEach(() => {
    delete window.__RUNTARA_CONFIG__;
  });

  afterEach(() => {
    delete window.__RUNTARA_CONFIG__;
  });

  it('lists every agent when all agents are enabled', () => {
    renderPicker(snapshot(['http', 'openai', 'csv']));
    expect(screen.getByTestId('step-picker-agent-http')).toBeInTheDocument();
    expect(screen.getByTestId('step-picker-agent-openai')).toBeInTheDocument();
    expect(screen.getByTestId('step-picker-agent-csv')).toBeInTheDocument();
  });

  it('hides agents whose module is not in the allowlist', () => {
    // `openai` excluded → its picker entry must be absent. http + csv remain.
    renderPicker(snapshot(['http', 'csv']));
    expect(screen.getByTestId('step-picker-agent-http')).toBeInTheDocument();
    expect(screen.getByTestId('step-picker-agent-csv')).toBeInTheDocument();
    expect(
      screen.queryByTestId('step-picker-agent-openai')
    ).not.toBeInTheDocument();
  });

  it('hides every agent when the allowlist is empty', () => {
    renderPicker(snapshot([]));
    expect(
      screen.queryByTestId('step-picker-agent-http')
    ).not.toBeInTheDocument();
    expect(
      screen.queryByTestId('step-picker-agent-openai')
    ).not.toBeInTheDocument();
    expect(
      screen.queryByTestId('step-picker-agent-csv')
    ).not.toBeInTheDocument();
  });
});
