import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router';
import { beforeEach, describe, expect, it, vi } from 'vitest';

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
    layout: {
      id: 'root',
      columns: 1,
      items: [
        {
          id: 'root_i0',
          child: { id: 'n_orders', type: 'block', blockId: 'orders' },
        },
      ],
    },
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
  definition: {
    definitionVersion: 1,
    layout: { id: 'root', columns: 1, items: [] },
    filters: [],
    blocks: [],
  },
};

const datasetReport: ReportDto = {
  ...sampleReport,
  id: 'rep_dataset',
  definition: {
    ...sampleReport.definition,
    datasets: [
      {
        id: 'orders_ds',
        label: 'Orders',
        source: { schema: 'orders' },
        dimensions: [{ field: 'status', label: 'Status', type: 'string' }],
        measures: [
          { id: 'total', label: 'Total', op: 'count', format: 'number' },
        ],
      },
    ],
  },
};

const stagedReport: ReportDto = {
  ...sampleReport,
  id: 'rep_staged',
  definition: {
    definitionVersion: 1,
    layout: { id: 'root', items: [] },
    filters: [{ id: 'stage', label: 'Stage', type: 'text' }],
    blocks: [],
    views: [
      { id: 'stage_a', title: 'Stage A', layout: { id: 'a', items: [] } },
      { id: 'stage_b', title: 'Stage B', layout: { id: 'b', items: [] } },
      { id: 'stage_c', title: 'Stage C', layout: { id: 'c', items: [] } },
    ],
    viewGroups: [
      {
        id: 'approval',
        mode: 'stages',
        stages: [
          { viewId: 'stage_a', value: 'A' },
          { viewId: 'stage_b', value: 'B' },
          { viewId: 'stage_c', value: 'C' },
        ],
        currentFrom: { type: 'filter', filterId: 'stage' },
        access: 'through_current',
        followCurrentOnAdvance: true,
      },
    ],
  },
};

const fixtureReports: ReportDto[] = [
  sampleReport,
  emptyReport,
  datasetReport,
  stagedReport,
];

const useReportRenderMock = vi.hoisted(() => vi.fn());

vi.mock('../hooks/useReports', () => ({
  useReport: (reportId: string | undefined) => ({
    data: fixtureReports.find((report) => report.id === reportId) ?? null,
    isFetching: false,
  }),
  useReportRender: (...args: unknown[]) =>
    useReportRenderMock(...args) ?? {
      data: undefined,
      isFetching: false,
      refetch: vi.fn(),
    },
  useReportBlockData: () => ({
    data: undefined,
    isFetching: false,
    refetch: vi.fn(),
  }),
  useReportPreview: () => ({ data: undefined, isFetching: false }),
  useCreateReport: () => ({ isPending: false, mutateAsync: vi.fn() }),
  useUpdateReport: () => ({ isPending: false, mutateAsync: vi.fn() }),
  useDeleteReport: () => ({ isPending: false, mutateAsync: vi.fn() }),
  useValidateReport: () => ({
    isPending: false,
    mutateAsync: vi.fn(),
    reset: vi.fn(),
  }),
}));

vi.mock('@/features/objects/hooks/useObjectSchemas', () => ({
  useObjectSchemaDtosByConnectionIds: () => ({
    schemasByConnectionId: {
      conn_object_model_default: [
        {
          id: 'sch_orders',
          name: 'orders',
          columns: [
            { name: 'id', type: 'string' },
            { name: 'status', type: 'string' },
          ],
        },
      ],
    },
  }),
}));

vi.mock('@/features/objects/hooks/useObjectModelConnectionSelection', () => ({
  useObjectModelConnectionSelection: () => ({
    selectedConnectionId: 'conn_object_model_default',
    selectedConnection: {
      id: 'conn_object_model_default',
      title: 'Object Model Postgres',
      integrationId: 'postgres',
      defaultFor: ['object_model'],
    },
    connections: [
      {
        id: 'conn_object_model_default',
        title: 'Object Model Postgres',
        integrationId: 'postgres',
        defaultFor: ['object_model'],
      },
    ],
    isLoading: false,
    setSelectedConnectionId: vi.fn(),
    connectionQuery: '?connectionId=conn_object_model_default',
  }),
}));

vi.mock('@/features/objects/components/ObjectModelConnectionSelector', () => ({
  ObjectModelConnectionSelector: () => (
    <div data-testid="object-model-connection-selector" />
  ),
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
          <Route
            path="/reports/:reportId"
            element={
              <>
                <ReportPage />
                <LocationProbe />
              </>
            }
          />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
}

function LocationProbe() {
  const location = useLocation();
  return <output data-testid="location-search">{location.search}</output>;
}

beforeEach(() => {
  useReportRenderMock.mockReset();
});

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
    expect(screen.getByRole('button', { name: 'Delete' })).toBeInTheDocument();
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

  it('withholds Explore from a report with no semantic dataset', async () => {
    renderAt(`/reports/${sampleReport.id}`);

    await waitFor(() => {
      expect(screen.getByText('Orders')).toBeInTheDocument();
    });
    // Following it would land on a page that can only say Explore is
    // unavailable here, so the action should not be offered at all.
    expect(screen.queryByRole('button', { name: 'Explore' })).toBeNull();
    // The sibling header actions are unaffected.
    expect(screen.getByRole('button', { name: 'Print' })).toBeInTheDocument();
  });

  it('offers Explore once the report exposes a dataset', async () => {
    renderAt(`/reports/${datasetReport.id}`);

    await waitFor(() => {
      expect(screen.getByText('Orders')).toBeInTheDocument();
    });
    expect(screen.getByRole('button', { name: 'Explore' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Explore' })).toHaveAttribute(
      'href',
      `/reports/${datasetReport.id}/explore`
    );
  });

  it('replaces an inaccessible future-stage URL with the server-resolved current view', async () => {
    useReportRenderMock.mockReturnValue({
      data: {
        success: true,
        report: { id: stagedReport.id, definitionVersion: 1 },
        resolvedFilters: { stage: 'B' },
        blocks: {},
        navigation: {
          requestedViewId: 'stage_c',
          activeViewId: 'stage_b',
          group: {
            id: 'approval',
            mode: 'stages',
            currentViewId: 'stage_b',
            accessibleViewIds: ['stage_a', 'stage_b'],
          },
        },
        errors: [],
      },
      isFetching: false,
      refetch: vi.fn(),
    });

    renderAt(`/reports/${stagedReport.id}?view=stage_c`);

    await waitFor(() => {
      expect(screen.getByTestId('location-search')).toHaveTextContent(
        '?view=stage_b'
      );
    });
    expect(useReportRenderMock).toHaveBeenCalledWith(
      stagedReport.id,
      expect.objectContaining({ viewId: 'stage_c' }),
      true
    );
  });

  it('follows a newly persisted current stage when refreshing from the old current view', async () => {
    useReportRenderMock.mockImplementation(function useMockReportRender(
      _reportId: string,
      request: { viewId?: string }
    ) {
      const [currentStage, setCurrentStage] = useState<'B' | 'C'>('B');
      const responseFor = (stage: 'B' | 'C') => ({
        success: true,
        report: { id: stagedReport.id, definitionVersion: 1 },
        resolvedFilters: { stage },
        blocks: {},
        navigation: {
          requestedViewId: request.viewId,
          activeViewId:
            stage === 'C' && request.viewId === 'stage_c'
              ? 'stage_c'
              : 'stage_b',
          group: {
            id: 'approval',
            mode: 'stages' as const,
            currentViewId: stage === 'B' ? 'stage_b' : 'stage_c',
            accessibleViewIds:
              stage === 'B'
                ? ['stage_a', 'stage_b']
                : ['stage_a', 'stage_b', 'stage_c'],
          },
        },
        errors: [],
      });
      return {
        data: responseFor(currentStage),
        isFetching: false,
        refetch: async () => {
          const next = responseFor('C');
          setCurrentStage('C');
          return { data: next };
        },
      };
    });

    renderAt(`/reports/${stagedReport.id}?view=stage_b`);
    await waitFor(() =>
      expect(screen.getByTestId('location-search')).toHaveTextContent(
        '?view=stage_b'
      )
    );

    await userEvent.click(screen.getByRole('button', { name: 'Refresh' }));

    await waitFor(() =>
      expect(screen.getByTestId('location-search')).toHaveTextContent(
        '?view=stage_c'
      )
    );
    await waitFor(() =>
      expect(useReportRenderMock).toHaveBeenLastCalledWith(
        stagedReport.id,
        expect.objectContaining({ viewId: 'stage_c' }),
        true
      )
    );
  });
});
