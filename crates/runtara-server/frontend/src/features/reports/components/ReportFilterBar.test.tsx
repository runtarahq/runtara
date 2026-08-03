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

function renderBar(
  values: Record<string, unknown>,
  visibleBlockIds: Set<string> | null = null,
  reportDefinition: ReportDefinition = definition
) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <ReportFilterBar
        reportId="report-1"
        definition={reportDefinition}
        values={values}
        onChange={vi.fn()}
        visibleBlockIds={visibleBlockIds}
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
    // The filter picker is built on cmdk, which scrolls its active item into
    // view. jsdom has no layout, so the method does not exist.
    Element.prototype.scrollIntoView = vi.fn();
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

  it('hides filters that do not apply to blocks in the active view', () => {
    const hidden = renderBar({ status: 'open' }, new Set(['detail-card']));
    expect(
      screen.queryByRole('button', { name: /Status:/i })
    ).not.toBeInTheDocument();
    hidden.unmount();

    renderBar({ status: 'open' }, new Set(['table']));
    expect(
      screen.getByRole('button', { name: /Status:/i })
    ).toBeInTheDocument();
  });

  it('shows a filter a visible block reaches through its source condition', () => {
    // The report editor documents an empty `appliesTo` as "the filter targets
    // all blocks via their source's condition". Treating it as "targets
    // nothing" hid the control, so the block gated on it could never be
    // populated.
    const conditionDefinition = {
      filters: [
        {
          id: 'period',
          label: 'Period',
          type: 'time_range',
          appliesTo: [],
        },
      ],
      blocks: [
        {
          id: 'trend',
          source: {
            condition: {
              op: 'AND',
              arguments: [
                {
                  op: 'GTE',
                  arguments: [
                    'snapshot_date',
                    { filter: 'period', path: 'from' },
                  ],
                },
              ],
            },
          },
        },
      ],
    } as unknown as ReportDefinition;

    renderBar({}, new Set(['trend']), conditionDefinition);

    // An unset filter lives behind the "+ Filter" picker — which is precisely
    // where Period never appeared, leaving it impossible to set.
    fireEvent.click(screen.getByRole('button', { name: /^Filter$/i }));
    expect(screen.getByText('Period')).toBeInTheDocument();
  });

  it('keeps that filter hidden when the block using it is not in view', () => {
    const conditionDefinition = {
      filters: [
        { id: 'period', label: 'Period', type: 'time_range', appliesTo: [] },
      ],
      blocks: [
        {
          id: 'trend',
          source: {
            condition: {
              op: 'GTE',
              arguments: ['snapshot_date', { filter: 'period', path: 'from' }],
            },
          },
        },
      ],
    } as unknown as ReportDefinition;

    renderBar({}, new Set(['something-else']), conditionDefinition);

    // No visible block references it, so the bar renders nothing at all.
    expect(
      screen.queryByRole('button', { name: /^Filter$/i })
    ).not.toBeInTheDocument();
    expect(screen.queryByText('Period')).not.toBeInTheDocument();
  });

  it('survives a definition that carries no blocks', () => {
    // The FE type marks `blocks` non-optional, but definitions reaching this
    // component do not always include it.
    const noBlocks = {
      filters: [
        { id: 'period', label: 'Period', type: 'time_range', appliesTo: [] },
      ],
    } as unknown as ReportDefinition;

    expect(() => renderBar({}, new Set(['trend']), noBlocks)).not.toThrow();
  });
});

describe('ReportFilterBar time range filters', () => {
  beforeAll(() => {
    vi.stubGlobal(
      'ResizeObserver',
      class {
        observe() {}
        unobserve() {}
        disconnect() {}
      }
    );
    Element.prototype.scrollIntoView = vi.fn();
  });
  beforeEach(() => vi.clearAllMocks());

  const timeRangeDefinition = {
    filters: [{ id: 'period', label: 'Period', type: 'time_range' }],
  } as unknown as ReportDefinition;

  function renderTimeRangeBar(value: unknown) {
    const onChange = vi.fn();
    const queryClient = new QueryClient({
      defaultOptions: { queries: { retry: false } },
    });
    render(
      <QueryClientProvider client={queryClient}>
        <ReportFilterBar
          reportId="report-1"
          definition={timeRangeDefinition}
          values={{ period: value }}
          onChange={onChange}
        />
      </QueryClientProvider>
    );
    return onChange;
  }

  const absoluteJuly = {
    from: '2026-07-01T00:00:00.000Z',
    to: '2026-08-01T00:00:00.000Z',
  };

  it('still commits presets as plain strings', () => {
    const onChange = renderTimeRangeBar('today');

    fireEvent.click(screen.getByRole('button', { name: /Period:/i }));
    fireEvent.click(screen.getByText('Last 7 days'));

    expect(onChange).toHaveBeenCalledWith('period', 'last_7_days');
  });

  it('commits a custom range as UTC day boundaries with an exclusive end', () => {
    const onChange = renderTimeRangeBar('today');

    fireEvent.click(screen.getByRole('button', { name: /Period:/i }));
    fireEvent.click(screen.getByText('Custom range'));

    // A half-filled draft must not render the report against an open window.
    fireEvent.change(screen.getByLabelText('From'), {
      target: { value: '2026-07-01' },
    });
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByText('Pick both dates to apply.')).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText('To'), {
      target: { value: '2026-07-31' },
    });
    expect(onChange).toHaveBeenCalledWith('period', absoluteJuly);
  });

  it('rejects an end date before the start date instead of committing', () => {
    const onChange = renderTimeRangeBar('today');

    fireEvent.click(screen.getByRole('button', { name: /Period:/i }));
    fireEvent.click(screen.getByText('Custom range'));
    fireEvent.change(screen.getByLabelText('From'), {
      target: { value: '2026-07-31' },
    });
    fireEvent.change(screen.getByLabelText('To'), {
      target: { value: '2026-07-01' },
    });

    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByRole('alert')).toHaveTextContent(
      'End date is before start date.'
    );
  });

  it('summarizes an absolute value as its date range on the chip', () => {
    renderTimeRangeBar(absoluteJuly);

    const format = new Intl.DateTimeFormat(undefined, {
      dateStyle: 'medium',
      timeZone: 'UTC',
    });
    const label = `${format.format(new Date('2026-07-01T00:00:00Z'))} – ${format.format(new Date('2026-07-31T00:00:00Z'))}`;
    expect(
      screen.getByRole('button', { name: `Period: ${label}` })
    ).toBeInTheDocument();
  });

  it('reopens an absolute value in custom mode with the days pre-filled', () => {
    renderTimeRangeBar(absoluteJuly);

    fireEvent.click(screen.getByRole('button', { name: /Period:/i }));

    expect(screen.getByLabelText('From')).toHaveValue('2026-07-01');
    expect(screen.getByLabelText('To')).toHaveValue('2026-07-31');
  });
});
