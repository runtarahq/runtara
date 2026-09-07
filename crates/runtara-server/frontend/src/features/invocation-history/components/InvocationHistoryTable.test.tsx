import { useState } from 'react';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { ExecutionHistoryFilters } from '../types';
import { InvocationHistoryTable } from './InvocationHistoryTable';

const query = vi.hoisted(() => vi.fn());
vi.mock('@/shared/hooks/api', () => ({ useTableQuery: query }));
vi.mock('../queries', () => ({ getAllExecutions: vi.fn() }));
vi.mock('./InvocationHistoryColumns', () => ({ invocationHistoryColumns: [] }));
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
      data: [{ instanceId: 'server-result' }],
      totalPages: 8,
      totalElements: 73,
      isFetching: false,
    }));
    render(<Harness />);
    fireEvent.click(screen.getByText('Next page'));
    const params = () => query.mock.lastCall![0].queryKey.at(-1);
    expect(params().pageIndex).toBe(1);
    fireEvent.change(screen.getByPlaceholderText('Search executions…'), {
      target: { value: 'Order/12 [done]' },
    });
    await waitFor(() =>
      expect(params().filters.search).toBe('Order/12 [done]')
    );
    expect(params().pageIndex).toBe(0);
    // No second client-side filter should discard a backend match.
    expect(screen.getByText('server-result')).toBeInTheDocument();
    expect(
      screen.getByText('73 executions · 1 on this page')
    ).toBeInTheDocument();
    fireEvent.click(screen.getByText('Next page'));
    expect(params().pageIndex).toBe(1);
    expect(params().filters.search).toBe('Order/12 [done]');
  });
  it('restores search when navigation changes the URL filters', async () => {
    query.mockImplementation(() => ({
      data: [],
      totalPages: 0,
      totalElements: 0,
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
    expect(screen.getByPlaceholderText('Search executions…')).toHaveValue(
      'Restored/12'
    );
    await new Promise((resolve) => setTimeout(resolve, 350));
    expect(onFiltersChange).not.toHaveBeenCalled();
  });
});
