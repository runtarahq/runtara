import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';

import type { ReportDefinition } from '../types';
import { resolveReportFilterOptions } from '../queries';
import { ReportFilterBar } from './ReportFilterBar';

vi.mock('react-oidc-context', () => ({
  useAuth: () => ({ user: { access_token: 'token' } }),
}));

vi.mock('../queries', () => ({
  resolveReportFilterOptions: vi.fn(),
}));

const definition = {
  filters: [
    {
      id: 'status',
      label: 'Status',
      type: 'select',
      options: { source: 'object_model' },
      appliesTo: [{ blockId: 'table' }],
    },
  ],
} as unknown as ReportDefinition;

function renderBar(values: Record<string, unknown>) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <ReportFilterBar
        reportId="report-1"
        definition={definition}
        values={values}
        onChange={vi.fn()}
      />
    </QueryClientProvider>
  );
}

describe('ReportFilterBar dynamic options', () => {
  beforeAll(() => {
    vi.stubGlobal(
      'ResizeObserver',
      class {
        observe() {}
        unobserve() {}
        disconnect() {}
      }
    );
  });
  beforeEach(() => vi.clearAllMocks());

  it('supplies the shared OptionResolver from the production report API', async () => {
    vi.mocked(resolveReportFilterOptions).mockResolvedValue({
      success: true,
      filter: { id: 'status' },
      page: { hasNextPage: false, offset: 0, size: 1, totalCount: 1 },
      options: [{ value: 'open', label: 'Open', count: 3 }],
    });
    renderBar({ status: 'open', company: 'acme' });

    fireEvent.click(screen.getByRole('button', { name: /Status:/i }));
    await waitFor(() => expect(resolveReportFilterOptions).toHaveBeenCalled());
    expect(resolveReportFilterOptions).toHaveBeenCalledWith(
      'token',
      'report-1',
      'status',
      expect.objectContaining({
        filters: { status: 'open', company: 'acme' },
        limit: 200,
      }),
      expect.any(AbortSignal)
    );
    expect(await screen.findByText('Open (3)')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /Status:/i }));
    fireEvent.click(screen.getByRole('button', { name: /Status:/i }));
    await screen.findByText('Open (3)');
    expect(resolveReportFilterOptions).toHaveBeenCalledTimes(1);
  });

  it('shows domain option failures instead of silently rendering an empty list', async () => {
    vi.mocked(resolveReportFilterOptions).mockRejectedValue(
      new Error('Option provider unavailable')
    );
    renderBar({ status: 'open' });

    fireEvent.click(screen.getByRole('button', { name: /Status:/i }));
    expect(
      await screen.findByText('Option provider unavailable')
    ).toBeInTheDocument();
  });
});
