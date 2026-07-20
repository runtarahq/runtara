import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { ReportBlockHost } from './ReportBlockHost';
import type { ReportBlockDefinition } from '../types';

vi.mock('react-oidc-context', () => ({
  useAuth: () => ({ user: { access_token: 'test-token' } }),
}));

const getReportBlockData = vi.fn();

vi.mock('../queries', async () => {
  const actual = await vi.importActual<Record<string, unknown>>('../queries');
  return {
    ...actual,
    getReportBlockData: (...args: unknown[]) => getReportBlockData(...args),
  };
});

// Mirror main.tsx defaults exactly — this is what production runs with.
// `useReportBlockData` overrides retry/retryDelay per-query on top of these.
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

// A lazy tile: needsBlockFetch === true, so it fetches its own data instead of
// using the report-level render payload. The backend omits lazy blocks from
// that payload, so there is no initialResult to fall back on.
const lazyMetric: ReportBlockDefinition = {
  id: 'revenue',
  type: 'metric',
  title: 'Revenue',
  lazy: true,
  source: { schema: 'orders', mode: 'aggregate' },
} as ReportBlockDefinition;

const networkError = () =>
  Object.assign(new Error('Network Error'), { code: 'ERR_NETWORK' });

const httpError = (status: number) =>
  Object.assign(new Error(`Request failed with status ${status}`), {
    response: { status, data: { message: 'Unknown block' } },
  });

beforeEach(() => {
  getReportBlockData.mockReset();
  // Every lazy block is immediately "visible" in jsdom.
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

function renderTile(client = makeClient()) {
  return render(
    <QueryClientProvider client={client}>
      <ReportBlockHost reportId="rep_abc" block={lazyMetric} filters={{}} />
    </QueryClientProvider>
  );
}

const skeleton = () => screen.queryByLabelText('Loading report block');

describe('lazy tile with a transient failure', () => {
  it('self-heals with no user action once the network recovers', async () => {
    // Fails once, then succeeds — a dropped connection.
    getReportBlockData
      .mockRejectedValueOnce(networkError())
      .mockResolvedValue({
        blockType: 'metric',
        status: 'ok',
        data: { value: 42 },
      });

    renderTile();

    // No remount, no Retry click, no page reload — the retry policy alone
    // brings it back.
    await waitFor(() => expect(screen.queryByText('42')).not.toBeNull(), {
      timeout: 10000,
    });
    expect(screen.queryByRole('button', { name: /retry/i })).toBeNull();
  }, 20000);

  it('gives up on an error surface rather than an endless skeleton', async () => {
    getReportBlockData.mockRejectedValue(networkError());

    renderTile();

    // 1 initial attempt + 3 retries, then the budget is spent.
    await waitFor(
      () => expect(getReportBlockData).toHaveBeenCalledTimes(4),
      { timeout: 15000 }
    );

    await waitFor(
      () =>
        expect(
          screen.queryByRole('button', { name: /retry/i })
        ).not.toBeNull(),
      { timeout: 5000 }
    );
    expect(skeleton()).toBeNull();
  }, 30000);
});

describe('lazy tile with a deterministic failure', () => {
  it('surfaces the error immediately without burning retries', async () => {
    getReportBlockData.mockRejectedValue(httpError(404));

    renderTile();

    await waitFor(
      () =>
        expect(
          screen.queryByRole('button', { name: /retry/i })
        ).not.toBeNull(),
      { timeout: 5000 }
    );

    // A 404 will never succeed — retrying it only delays the error.
    expect(getReportBlockData).toHaveBeenCalledTimes(1);
    expect(skeleton()).toBeNull();
  }, 15000);

  it('retries a 500, which may be load-related', async () => {
    getReportBlockData
      .mockRejectedValueOnce(httpError(500))
      .mockResolvedValue({
        blockType: 'metric',
        status: 'ok',
        data: { value: 7 },
      });

    renderTile();

    await waitFor(() => expect(screen.queryByText('7')).not.toBeNull(), {
      timeout: 10000,
    });
  }, 20000);
});
