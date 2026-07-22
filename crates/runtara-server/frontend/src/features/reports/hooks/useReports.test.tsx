import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { useAuth } from 'react-oidc-context';
import type { ReportRenderResponse } from '../types';
import { useReportRender } from './useReports';

const queryMocks = vi.hoisted(() => ({
  renderReport: vi.fn(),
}));

vi.mock('react-oidc-context', () => ({
  useAuth: vi.fn(),
}));

vi.mock('../queries', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../queries')>()),
  renderReport: queryMocks.renderReport,
}));

const renderResponse = (definitionVersion: number): ReportRenderResponse => ({
  success: true,
  blocks: {},
  resolvedFilters: {},
  report: { id: 'report-1', definitionVersion },
});

describe('useReportRender', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(useAuth).mockReturnValue({
      user: { access_token: 'test-token' },
    } as ReturnType<typeof useAuth>);
  });

  it('refreshes a still-fresh cached render when the report is reopened', async () => {
    const queryClient = new QueryClient({
      defaultOptions: {
        queries: {
          staleTime: 5 * 60 * 1000,
          retry: false,
        },
      },
    });
    const wrapper = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>
    );
    const request = { filters: {} };
    queryMocks.renderReport.mockResolvedValueOnce(renderResponse(1));

    const firstView = renderHook(
      () => useReportRender('report-1', request, true),
      { wrapper }
    );
    await waitFor(() =>
      expect(firstView.result.current.data).toEqual(renderResponse(1))
    );
    firstView.unmount();

    queryMocks.renderReport.mockResolvedValueOnce(renderResponse(2));
    const reopenedView = renderHook(
      () => useReportRender('report-1', request, true),
      { wrapper }
    );

    await waitFor(() => {
      expect(queryMocks.renderReport).toHaveBeenCalledTimes(2);
      expect(reopenedView.result.current.data).toEqual(renderResponse(2));
    });
    reopenedView.unmount();
    queryClient.clear();
  });
});
