import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import type { ReportBlockDefinition, ReportBlockResult } from '../../types';
import { TableBlock } from './TableBlock';

vi.mock('react-oidc-context', () => ({
  useAuth: () => ({ user: { access_token: 'test-token' } }),
}));

function renderTableBlock(
  block: ReportBlockDefinition,
  result: ReportBlockResult,
  options: {
    onPageChange?: (offset: number, size: number) => void;
  } = {}
) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });

  return render(
    <QueryClientProvider client={queryClient}>
      <TableBlock
        reportId="report_1"
        block={block}
        result={result}
        sort={block.table?.defaultSort ?? []}
        filters={{}}
        blockFilters={{}}
        onPageChange={options.onPageChange ?? (() => {})}
        onSortChange={() => {}}
      />
    </QueryClientProvider>
  );
}

describe('TableBlock maxChars sizing', () => {
  it('uses maxChars as both a text cutoff and a table column width hint', () => {
    const block: ReportBlockDefinition = {
      id: 'records',
      type: 'table',
      source: { schema: 'WorkflowButtonDemoItem', mode: 'filter' },
      table: {
        columns: [
          { field: 'code', label: 'Code' },
          { field: 'title', label: 'Work item', maxChars: 12 },
        ],
      },
    };
    const result: ReportBlockResult = {
      type: 'table',
      status: 'ready',
      data: {
        columns: [
          { key: 'code', label: 'Code' },
          { key: 'title', label: 'Work item', maxChars: 12 },
        ],
        rows: [
          {
            code: 'WB-001',
            title: 'Recalculate eligibility score',
          },
        ],
      },
    };

    const { container } = renderTableBlock(block, result);

    expect(screen.getByText('Recalculate...')).toBeInTheDocument();
    expect(container.querySelector('table')).toHaveClass('table-fixed');
    expect(container.querySelector('colgroup')).not.toBeNull();
    expect(container.querySelectorAll('col')[1]).toHaveStyle({
      width: 'calc(15ch + 1rem)',
      maxWidth: 'calc(15ch + 1rem)',
    });
  });

  it('fills the table with a spacer instead of stretching cutoff columns', () => {
    const block: ReportBlockDefinition = {
      id: 'records',
      type: 'table',
      source: { schema: 'WorkflowButtonDemoItem', mode: 'filter' },
      table: {
        columns: [{ field: 'title', label: 'Work item', maxChars: 12 }],
      },
    };
    const result: ReportBlockResult = {
      type: 'table',
      status: 'ready',
      data: {
        columns: [{ key: 'title', label: 'Work item', maxChars: 12 }],
        rows: [
          {
            title: 'Recalculate eligibility score',
          },
        ],
      },
    };

    const { container } = renderTableBlock(block, result);

    const table = container.querySelector('table');
    expect(table).toHaveClass('table-fixed');
    expect(table).toHaveClass('w-full');
    expect(table).not.toHaveClass('w-max');
    const columns = container.querySelectorAll('col');
    expect(columns).toHaveLength(2);
    expect(columns[0]).toHaveStyle({
      width: 'calc(15ch + 1rem)',
      maxWidth: 'calc(15ch + 1rem)',
    });
    expect(columns[1]).toHaveAttribute('aria-hidden', 'true');

    const headerCells = container.querySelectorAll('th');
    expect(headerCells).toHaveLength(2);
    expect(headerCells[1]).toHaveAttribute('aria-hidden', 'true');
  });
});

describe('TableBlock pagination', () => {
  it('renders full pagination controls when total count is known', () => {
    const block: ReportBlockDefinition = {
      id: 'records',
      type: 'table',
      source: { schema: 'WorkflowButtonDemoItem', mode: 'filter' },
      table: {
        columns: [{ field: 'title', label: 'Work item' }],
        pagination: {
          defaultPageSize: 25,
          allowedPageSizes: [25, 50, 100],
        },
      },
    };
    const result: ReportBlockResult = {
      type: 'table',
      status: 'ready',
      data: {
        columns: [{ key: 'title', label: 'Work item' }],
        rows: [{ title: 'Recalculate eligibility score' }],
        page: {
          offset: 50,
          size: 25,
          totalCount: 120,
          hasNextPage: true,
        },
      },
    };
    const onPageChange = vi.fn();

    renderTableBlock(block, result, { onPageChange });

    expect(screen.getByText('51-75 of 120')).toBeInTheDocument();
    expect(screen.getByText('Page 3 of 5')).toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: /First/i }));
    fireEvent.click(screen.getByRole('button', { name: /Previous/i }));
    fireEvent.click(screen.getByRole('button', { name: /Next/i }));
    fireEvent.click(screen.getByRole('button', { name: /Last/i }));

    expect(onPageChange).toHaveBeenNthCalledWith(1, 0, 25);
    expect(onPageChange).toHaveBeenNthCalledWith(2, 25, 25);
    expect(onPageChange).toHaveBeenNthCalledWith(3, 75, 25);
    expect(onPageChange).toHaveBeenNthCalledWith(4, 100, 25);
  });
});
