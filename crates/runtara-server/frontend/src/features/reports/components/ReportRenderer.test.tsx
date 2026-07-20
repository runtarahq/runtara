import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ReportRenderer } from './ReportRenderer';
import type { ReportDefinition, ReportRenderResponse } from '../types';

vi.mock('react-oidc-context', () => ({
  useAuth: () => ({ user: { access_token: 'test-token' } }),
}));

// The renderer suspends on the report-DSL WASM bundle; stub it out.
vi.mock('../hooks/useReportDsl', () => ({
  useReportDsl: () => undefined,
}));

const getReportBlockData = vi.fn();

vi.mock('../queries', async () => {
  const actual = await vi.importActual<Record<string, unknown>>('../queries');
  return {
    ...actual,
    getReportBlockData: (...args: unknown[]) => getReportBlockData(...args),
  };
});

const TILE_COUNT = 12;
// The tiles whose request fails. These use a deterministic (non-retryable)
// failure so the dashboard settles fast — `ReportBlockHost.test.tsx` covers
// the transient/retry path in detail.
const FAILING_TILES = new Set(['tile_3', 'tile_7', 'tile_10']);

function buildDefinition(): ReportDefinition {
  const blocks = Array.from({ length: TILE_COUNT }, (_, i) => ({
    id: `tile_${i}`,
    type: 'metric' as const,
    title: `Tile ${i}`,
    // A wide dashboard: heavy tiles are authored lazy so the initial
    // report render stays fast. Lazy blocks are excluded from the
    // report-level render payload, so they have no fallback result.
    lazy: true,
    source: { schema: 'orders', mode: 'aggregate' as const },
  }));

  return {
    definitionVersion: 1,
    layout: {
      id: 'root',
      columns: 4,
      items: blocks.map((block, i) => ({
        id: `root_i${i}`,
        child: { id: `n_${block.id}`, type: 'block' as const, blockId: block.id },
      })),
    },
    filters: [],
    blocks,
  } as unknown as ReportDefinition;
}

const definition = buildDefinition();

// The report-level render response: lazy blocks are absent, exactly as the
// backend's `requested_blocks` filter produces.
const renderResponse: ReportRenderResponse = {
  success: true,
  report: { id: 'rep_abc', definitionVersion: 1 },
  resolvedFilters: {},
  blocks: {},
  errors: [],
} as unknown as ReportRenderResponse;

beforeEach(() => {
  getReportBlockData.mockReset();
  vi.stubGlobal(
    'IntersectionObserver',
    class {
      constructor(private cb: (entries: unknown[]) => void) {
        setTimeout(() => this.cb([{ isIntersecting: true }]), 0);
      }
      observe() {}
      disconnect() {}
      unobserve() {}
    }
  );
});

function makeClient() {
  return new QueryClient({
    defaultOptions: {
      queries: {
        staleTime: 1000 * 60 * 5,
        gcTime: 1000 * 60 * 10,
        retry: 1,
        refetchOnWindowFocus: false,
      },
    },
  });
}

describe('wide dashboard under latency + flaky connections', () => {
  it('renders every tile, not a permanent skeleton on the ones that failed', async () => {
    getReportBlockData.mockImplementation(
      async (_token: unknown, context: { queryKey: readonly unknown[] }) => {
        const blockId = context.queryKey[4] as string;
        // High latency on every tile.
        await new Promise((resolve) => setTimeout(resolve, 30));
        if (FAILING_TILES.has(blockId)) {
          throw Object.assign(new Error('Request failed with status 404'), {
            response: { status: 404, data: { message: 'Unknown block' } },
          });
        }
        return { blockType: 'metric', status: 'ok', data: { value: 100 } };
      }
    );

    render(
      <QueryClientProvider client={makeClient()}>
        <ReportRenderer
          reportId="rep_abc"
          definition={definition}
          renderResponse={renderResponse}
          filters={{}}
        />
      </QueryClientProvider>
    );

    // Healthy tiles settle.
    await waitFor(
      () =>
        expect(screen.getAllByText('100')).toHaveLength(
          TILE_COUNT - FAILING_TILES.size
        ),
      { timeout: 10000 }
    );

    // The failed tiles must offer a way back without a page reload.
    await waitFor(
      () =>
        expect(screen.getAllByRole('button', { name: /retry/i })).toHaveLength(
          FAILING_TILES.size
        ),
      { timeout: 10000 }
    );

    const stuck = screen.queryAllByLabelText('Loading report block');
    expect(
      stuck.length,
      `${stuck.length} tile(s) are still showing a loading skeleton with no error and no retry affordance`
    ).toBe(0);

    // A 404 is deterministic — no retries were burned on it.
    expect(getReportBlockData).toHaveBeenCalledTimes(TILE_COUNT);
  }, 20000);
});
