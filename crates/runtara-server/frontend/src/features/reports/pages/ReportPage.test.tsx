import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router';
import { describe, expect, it, vi } from 'vitest';

import { ReportPage } from './ReportPage';
import type { ReportDto } from '../types';

vi.mock('react-oidc-context', () => ({
  useAuth: () => ({ user: { access_token: 'test-token' } }),
}));

const sampleReport: ReportDto = {
  id: 'rep_abc',
  slug: 'sample',
  name: 'Sample report',
  description: null,
  tags: [],
  status: 'published',
  definitionVersion: 1,
  createdAt: '2026-05-14T00:00:00Z',
  updatedAt: '2026-05-14T00:00:00Z',
  definition: {
    definitionVersion: 1,
    filters: [],
    blocks: [
      {
        id: 'orders',
        type: 'table',
        title: 'Orders',
        source: { schema: 'orders', mode: 'filter' },
        table: {
          columns: [
            { field: 'id', label: 'Order Id' },
            { field: 'status', label: 'Status', format: 'pill' },
          ],
        },
      },
    ],
  },
};

const emptyReport: ReportDto = {
  ...sampleReport,
  id: 'rep_empty',
  definition: { definitionVersion: 1, filters: [], blocks: [] },
};

vi.mock('../hooks/useReports', () => ({
  useReport: (reportId: string | undefined) => ({
    data:
      reportId === sampleReport.id
        ? sampleReport
        : reportId === emptyReport.id
          ? emptyReport
          : null,
    isFetching: false,
  }),
  useReportRender: () => ({
    data: undefined,
    isFetching: false,
    refetch: vi.fn(),
  }),
  useReportBlockData: () => ({
    data: undefined,
    isFetching: false,
    refetch: vi.fn(),
  }),
  useReportPreview: () => ({ data: undefined, isFetching: false }),
  useCreateReport: () => ({ isPending: false, mutateAsync: vi.fn() }),
  useUpdateReport: () => ({ isPending: false, mutateAsync: vi.fn() }),
  useValidateReport: () => ({
    isPending: false,
    mutateAsync: vi.fn(),
    reset: vi.fn(),
  }),
}));

vi.mock('@/features/objects/hooks/useObjectSchemas', () => ({
  useObjectSchemaDtos: () => ({
    data: [
      {
        id: 'sch_orders',
        name: 'orders',
        columns: [
          { name: 'id', type: 'string' },
          { name: 'status', type: 'string' },
        ],
      },
    ],
  }),
}));

function renderAt(path: string) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={[path]}>
        <Routes>
          <Route path="/reports/new" element={<ReportPage />} />
          <Route path="/reports/:reportId" element={<ReportPage />} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
}

describe('ReportPage existing-report load', () => {
  it('renders the loaded report definition (not an empty wizard)', async () => {
    renderAt(`/reports/${sampleReport.id}`);

    // The wizard should mount with the saved table block, not the empty
    // "Add at least one block" state. The block title is the most reliable
    // signal it's not empty.
    await waitFor(() => {
      expect(screen.getByText('Orders')).toBeInTheDocument();
    });
    // Negative assertion: empty-state CTA shouldn't be present.
    expect(
      screen.queryByText('Add at least one block')
    ).not.toBeInTheDocument();
  });

  it('shows a friendly empty-state message when a viewed report has no blocks', async () => {
    renderAt(`/reports/${emptyReport.id}`);

    await waitFor(() => {
      expect(
        screen.getByText('This report has no content yet')
      ).toBeInTheDocument();
    });
    expect(screen.getByText(/Switch to edit mode/i)).toBeInTheDocument();
  });
});
