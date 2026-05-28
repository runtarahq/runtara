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

describe('TableBlock default sizing without explicit config', () => {
  it('applies table-fixed layout and a colgroup with widths even without maxChars', () => {
    const block: ReportBlockDefinition = {
      id: 'records',
      type: 'table',
      source: { schema: 'WorkflowButtonDemoItem', mode: 'filter' },
      table: {
        columns: [
          { field: 'sku', label: 'SKU' },
          { field: 'description', label: 'Description' },
          { field: 'qty', label: 'Qty' },
        ],
      },
    };
    const result: ReportBlockResult = {
      type: 'table',
      status: 'ready',
      data: {
        columns: [
          { key: 'sku', label: 'SKU' },
          { key: 'description', label: 'Description' },
          { key: 'qty', label: 'Qty' },
        ],
        rows: [
          { sku: 'WB-001', description: 'A reasonably long description', qty: 7 },
          { sku: 'WB-002', description: 'Another description with words', qty: 12 },
        ],
      },
    };

    const { container } = renderTableBlock(block, result);

    expect(container.querySelector('table')).toHaveClass('table-fixed');
    expect(container.querySelector('colgroup')).not.toBeNull();
    const cols = container.querySelectorAll('col');
    expect(cols.length).toBeGreaterThanOrEqual(3);
    // Short code column gets a bounded ch-based width.
    expect((cols[0] as HTMLElement).getAttribute('style')).toMatch(/ch/);
    // Long description column gets no explicit width — flexes.
    const descStyle = (cols[1] as HTMLElement).getAttribute('style');
    expect(descStyle === null || descStyle === '').toBe(true);
  });

  it('shrinks a column to header width when every sampled value is empty', () => {
    const block: ReportBlockDefinition = {
      id: 'records',
      type: 'table',
      source: { schema: 'WorkflowButtonDemoItem', mode: 'filter' },
      table: {
        columns: [
          { field: 'sku', label: 'SKU' },
          { field: 'original_cat', label: 'Original Cat' },
        ],
      },
    };
    const result: ReportBlockResult = {
      type: 'table',
      status: 'ready',
      data: {
        columns: [
          { key: 'sku', label: 'SKU' },
          { key: 'original_cat', label: 'Original Cat' },
        ],
        rows: [
          { sku: 'WB-001', original_cat: null },
          { sku: 'WB-002', original_cat: '' },
        ],
      },
    };

    const { container } = renderTableBlock(block, result);
    const cols = container.querySelectorAll('col');
    // The empty column collapses to a 1% width (browser shrinks to header).
    expect((cols[1] as HTMLElement).getAttribute('style')).toMatch(
      /width:\s*1%/
    );
  });

  it('renders an em-dash placeholder for null cell values', () => {
    const block: ReportBlockDefinition = {
      id: 'records',
      type: 'table',
      source: { schema: 'WorkflowButtonDemoItem', mode: 'filter' },
      table: {
        columns: [{ field: 'vendor', label: 'Vendor' }],
      },
    };
    const result: ReportBlockResult = {
      type: 'table',
      status: 'ready',
      data: {
        columns: [{ key: 'vendor', label: 'Vendor' }],
        rows: [{ vendor: null }, { vendor: 'Acme' }],
      },
    };

    const { container } = renderTableBlock(block, result);
    // Em-dash for null + the actual vendor name for the other row.
    const placeholders = container.querySelectorAll('[aria-label="No value"]');
    expect(placeholders.length).toBe(1);
    expect(placeholders[0].textContent).toBe('—');
    expect(screen.getByText('Acme')).toBeInTheDocument();
  });

  it('right-aligns numeric columns inferred from data even without an explicit format', () => {
    const block: ReportBlockDefinition = {
      id: 'records',
      type: 'table',
      source: { schema: 'WorkflowButtonDemoItem', mode: 'filter' },
      table: {
        columns: [
          { field: 'sku', label: 'SKU' },
          { field: 'quantity', label: 'Quantity' },
        ],
      },
    };
    const result: ReportBlockResult = {
      type: 'table',
      status: 'ready',
      data: {
        columns: [
          { key: 'sku', label: 'SKU' },
          { key: 'quantity', label: 'Quantity' },
        ],
        rows: [
          { sku: 'WB-001', quantity: 12 },
          { sku: 'WB-002', quantity: 5 },
          { sku: 'WB-003', quantity: 87 },
        ],
      },
    };

    const { container } = renderTableBlock(block, result);
    // The Quantity header is the second header cell (no selectable column).
    const headerCells = container.querySelectorAll('th');
    const quantityHeader = headerCells[1];
    expect(quantityHeader.className).toMatch(/text-right/);
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
