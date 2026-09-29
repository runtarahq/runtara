import { useState } from 'react';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { ExecutionHistoryFilters } from '../types';
import { InvocationHistoryTable } from './InvocationHistoryTable';

const query = vi.hoisted(() => vi.fn());
vi.mock('@/shared/hooks/api', () => ({ useCustomQuery: query }));
vi.mock('../queries', () => ({ getAllExecutions: vi.fn() }));
vi.mock('./OperationsRunColumns', () => ({ operationsRunColumns: () => [] }));
vi.mock('./RunRow', () => ({
  RunIdentity: () => null,
  RunDetails: () => null,
  RunDetailsToggle: () => null,
  RunTime: () => null,
  RunContext: () => null,
  RunActions: ({
    run,
    onReplay,
  }: {
    run: { instanceId: string };
    onReplay: (run: unknown) => void;
  }) => <button onClick={() => onReplay(run)}>Open replay</button>,
}));
vi.mock('@/features/operations/pages/shared', () => ({
  ReplayButton: ({ run }: { run: { id: string } }) => (
    <p>Replay snapshot: {run.id}</p>
  ),
  RefreshControls: ({ onRefresh }: { onRefresh: () => void }) => (
    <button onClick={onRefresh}>Refresh</button>
  ),
}));
vi.mock('./InvocationHistoryFilters', () => ({
  InvocationHistoryFilters: () => null,
  countActiveInvocationFilters: () => 0,
}));
vi.mock('@/shared/components/table', () => ({
  DataTable: ({ data }: { data: { instanceId: string }[] }) => (
    <div>
      {data.map((row) => (
        <span key={row.instanceId}>{row.instanceId}</span>
      ))}
    </div>
  ),
}));
vi.mock('@/shared/components/console', () => ({
  Breadcrumb: () => null,
  ConsoleTableShell: ({
    toolbar,
    footer,
    children,
  }: Record<string, React.ReactNode>) => (
    <div>
      {toolbar}
      {children}
      {footer}
    </div>
  ),
  ConsoleToolbar: ({ search }: { search: React.ReactNode }) => (
    <div>{search}</div>
  ),
  FilterPopover: () => null,
  ToolbarSearch: ({
    value,
    onChange,
    placeholder,
  }: {
    value: string;
    onChange: (value: string) => void;
    placeholder: string;
  }) => (
    <input
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder}
    />
  ),
  TableStatusFooter: ({ left, right }: Record<string, React.ReactNode>) => (
    <div>
      {left}
      {right}
    </div>
  ),
  TablePagination: ({
    pageIndex,
    onPageChange,
  }: {
    pageIndex: number;
    onPageChange: (page: number) => void;
  }) => <button onClick={() => onPageChange(pageIndex + 1)}>Next page</button>,
}));

function Harness() {
  const [filters, setFilters] = useState<ExecutionHistoryFilters>({
    sortBy: 'createdAt',
  });
  return (
    <InvocationHistoryTable filters={filters} onFiltersChange={setFilters} />
  );
}

describe('execution search pagination', () => {
  it('sends search in the query key, resets a later page, and trusts the server results and totals', async () => {
    query.mockImplementation(() => ({
      data: {
        content: [{ instanceId: 'server-result' }],
        totalPages: 8,
        totalElements: 73,
      },
      isFetching: false,
    }));
    render(<Harness />);
    fireEvent.click(screen.getByText('Next page'));
    const params = () => query.mock.lastCall![0].queryKey.at(-1);
    expect(params().pageIndex).toBe(1);
    fireEvent.change(screen.getByPlaceholderText('Search runs…'), {
      target: { value: 'Order/12 [done]' },
    });
    await waitFor(() =>
      expect(params().filters.search).toBe('Order/12 [done]')
    );
    expect(params().pageIndex).toBe(0);
    // No second client-side filter should discard a backend match.
    expect(screen.getByText('server-result')).toBeInTheDocument();
    expect(screen.getByText('73 runs · 1 on this page')).toBeInTheDocument();
    fireEvent.click(screen.getByText('Next page'));
    expect(params().pageIndex).toBe(1);
    expect(params().filters.search).toBe('Order/12 [done]');
  });
  it('restores search when navigation changes the URL filters', async () => {
    query.mockImplementation(() => ({
      data: { content: [], totalPages: 0, totalElements: 0 },
      isFetching: false,
    }));
    const onFiltersChange = vi.fn();
    const { rerender } = render(
      <InvocationHistoryTable
        filters={{ search: 'First' }}
        onFiltersChange={onFiltersChange}
      />
    );
    rerender(
      <InvocationHistoryTable
        filters={{ search: 'Restored/12' }}
        onFiltersChange={onFiltersChange}
      />
    );
    expect(screen.getByPlaceholderText('Search runs…')).toHaveValue(
      'Restored/12'
    );
    await new Promise((resolve) => setTimeout(resolve, 350));
    expect(onFiltersChange).not.toHaveBeenCalled();
  });
});

it('keeps stale rows visible, makes counts unavailable and allows manual refresh', () => {
  const refetch = vi.fn();
  query.mockReturnValue({
    data: {
      content: [{ instanceId: 'last-success' }],
      totalPages: 1,
      totalElements: 1,
      summary: { total: 1, counts: { failed: 1 } },
    },
    error: new Error('offline'),
    refetch,
  });
  render(<Harness />);
  expect(screen.getByText('last-success')).toBeInTheDocument();
  expect(screen.getByRole('alert')).toHaveTextContent('Showing stale data');
  expect(screen.getByRole('button', { name: 'Failed —' })).toBeInTheDocument();
  fireEvent.click(screen.getByText('Refresh'));
  expect(refetch).toHaveBeenCalledOnce();
  fireEvent.click(screen.getByRole('switch'));
  expect(query.mock.lastCall![0].refetchInterval).toBe(false);
});

it('keeps results when only counts fail and selecting a quick filter preserves the context', () => {
  query.mockReturnValue({
    data: {
      content: [],
      totalPages: 0,
      totalElements: 0,
      countsUnavailable: true,
    },
  });
  const onChange = vi.fn();
  render(
    <InvocationHistoryTable
      filters={{
        workflowId: 'w',
        range: '24h',
        dateBasis: 'completed',
        sortBy: 'completedAt',
      }}
      onFiltersChange={onChange}
    />
  );
  expect(screen.getByRole('alert')).toHaveTextContent(
    'Status counts unavailable'
  );
  fireEvent.click(screen.getByRole('button', { name: 'Waiting —' }));
  expect(onChange).toHaveBeenCalledWith({
    workflowId: 'w',
    range: '24h',
    dateBasis: 'completed',
    sortBy: 'completedAt',
    status: 'suspended',
  });
});

it('keeps Replay attached to its original run when refreshed rows change', () => {
  const filters = {};
  const row = {
    instanceId: 'original-run',
    workflowId: 'workflow',
    runLabel: 'ORDER-123',
  };
  query.mockReturnValue({ data: { content: [row], totalElements: 1 } });
  const { rerender } = render(
    <InvocationHistoryTable filters={filters} onFiltersChange={vi.fn()} />
  );
  fireEvent.click(screen.getByText('Open replay'));
  expect(query.mock.lastCall![0].refetchInterval).toBe(false);
  query.mockReturnValue({ data: { content: [], totalElements: 0 } });
  rerender(
    <InvocationHistoryTable filters={filters} onFiltersChange={vi.fn()} />
  );
  expect(screen.getByRole('dialog')).toHaveTextContent(
    'Replay snapshot: original-run'
  );
});
