import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import type { ReportBlockDefinition, ReportBlockResult } from '../../types';
import { TableBlock } from './TableBlock';
import { MAX_TEXT_PX, MIN_TEXT_PX } from './tableLayout';

function colWidthPx(col: Element): number {
  const style = (col as HTMLElement).getAttribute('style') ?? '';
  const match = style.match(/width:\s*(\d+(?:\.\d+)?)px/);
  expect(match, `expected a px width, got "${style}"`).not.toBeNull();
  return Number(match![1]);
}

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
  it('uses maxChars as a text cutoff and a width cap', () => {
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
    // maxChars 12 caps the width at ~15 glyphs (cutoff + ellipsis) — the
    // column is sized to its truncated content, never wider than the cap.
    const width = colWidthPx(container.querySelectorAll('col')[1]);
    expect(width).toBeLessThanOrEqual(160);
    expect(width).toBeGreaterThanOrEqual(MIN_TEXT_PX);
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
    expect(colWidthPx(columns[0])).toBeLessThanOrEqual(160);
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
    // 3 data columns + a trailing filler col that absorbs slack.
    expect(cols.length).toBe(4);
    // Each data column gets a bounded px width.
    expect(colWidthPx(cols[0])).toBeGreaterThan(0);
    expect(colWidthPx(cols[1])).toBeGreaterThan(0);
    expect(colWidthPx(cols[2])).toBeGreaterThan(0);
    // The last col is the aria-hidden filler (no width).
    expect((cols[3] as HTMLElement).getAttribute('aria-hidden')).toBe('true');
  });

  it('gives every data column an explicit bounded width — no auto/flex column (regression: 15000px-wide table)', () => {
    // Several long-text columns at once was the trigger: as auto/flex columns
    // with nowrap content under table-fixed + min-w-max, they each expanded to
    // full intrinsic width and blew the table up so only one column was
    // visible. Every data column must now resolve to a concrete `ch`/px width.
    const longCols = ['sku', 'description', 'vendor', 'effective'];
    const block: ReportBlockDefinition = {
      id: 'records',
      type: 'table',
      source: { schema: 'WorkflowButtonDemoItem', mode: 'filter' },
      table: {
        selectable: true,
        columns: longCols.map((field) => ({ field, label: field })),
      },
    };
    const result: ReportBlockResult = {
      type: 'table',
      status: 'ready',
      data: {
        columns: longCols.map((key) => ({ key, label: key })),
        rows: [
          {
            sku: '1 1/2in Sch 40 pipe',
            description: '1 1/2in Sch 40 pipe',
            vendor: 'SOUTHERN PIPE AND SUPPLY',
            effective: 'Schedule 40 PVC pipe, 1.5 inches in diameter',
          },
        ],
      },
    };

    const { container } = renderTableBlock(block, result);
    const cols = Array.from(container.querySelectorAll('col'));
    // select col + 4 data cols + filler.
    expect(cols.length).toBe(6);
    // The 4 data columns (indices 1..4) each carry a concrete bounded width.
    for (const idx of [1, 2, 3, 4]) {
      expect(colWidthPx(cols[idx])).toBeLessThanOrEqual(MAX_TEXT_PX);
    }
    // None of the data columns is left auto/flex (the regression cause).
    const flexCols = cols
      .slice(1, 5)
      .filter((c) => !((c as HTMLElement).getAttribute('style') ?? '').trim());
    expect(flexCols).toHaveLength(0);
  });

  it('sizes an all-empty column to its header instead of collapsing', () => {
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
    // Sized to a real (header-derived) width, never the old fragile 1%.
    expect(colWidthPx(cols[1])).toBeGreaterThanOrEqual(MIN_TEXT_PX);
    const emptyColStyle =
      (cols[1] as HTMLElement).getAttribute('style') ?? '';
    expect(emptyColStyle).not.toMatch(/1%/);
  });

  it('clamps a very long text column to the max width', () => {
    const block: ReportBlockDefinition = {
      id: 'records',
      type: 'table',
      source: { schema: 'WorkflowButtonDemoItem', mode: 'filter' },
      table: {
        columns: [{ field: 'effective', label: 'Effective' }],
      },
    };
    const result: ReportBlockResult = {
      type: 'table',
      status: 'ready',
      data: {
        columns: [{ key: 'effective', label: 'Effective' }],
        rows: [
          {
            effective:
              'A very long descriptive sentence that would otherwise force the column to take an enormous amount of horizontal space across the table',
          },
        ],
      },
    };

    const { container } = renderTableBlock(block, result);
    const cols = container.querySelectorAll('col');
    // MAX_TEXT_PX cap — keeps one long column from monopolizing the table.
    expect(colWidthPx(cols[0])).toBeLessThanOrEqual(MAX_TEXT_PX);
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
});

describe('TableBlock container fitting', () => {
  it('compresses flexible columns to the container width instead of overflowing', () => {
    // jsdom has no ResizeObserver; stub one that reports a narrow container
    // synchronously on observe so the fit pass runs.
    class ImmediateResizeObserver {
      private readonly callback: ResizeObserverCallback;
      constructor(callback: ResizeObserverCallback) {
        this.callback = callback;
      }
      observe() {
        this.callback(
          [{ contentRect: { width: 400 } } as ResizeObserverEntry],
          this as unknown as ResizeObserver
        );
      }
      unobserve() {}
      disconnect() {}
    }
    vi.stubGlobal('ResizeObserver', ImmediateResizeObserver);

    try {
      const longText =
        'A long descriptive sentence that would push this column to its cap';
      const block: ReportBlockDefinition = {
        id: 'records',
        type: 'table',
        source: { schema: 'WorkflowButtonDemoItem', mode: 'filter' },
        table: {
          columns: [
            { field: 'a', label: 'A' },
            { field: 'b', label: 'B' },
            { field: 'c', label: 'C' },
          ],
        },
      };
      const result: ReportBlockResult = {
        type: 'table',
        status: 'ready',
        data: {
          columns: [
            { key: 'a', label: 'A' },
            { key: 'b', label: 'B' },
            { key: 'c', label: 'C' },
          ],
          rows: [{ a: longText, b: longText, c: longText }],
        },
      };

      const { container } = renderTableBlock(block, result);
      const cols = Array.from(container.querySelectorAll('col')).slice(0, 3);
      const widths = cols.map(colWidthPx);
      // Uncompressed the three columns would want ~3 × MAX_TEXT_PX; the fit
      // pass shrinks them so the table fits the 400px container.
      const total = widths.reduce((acc, w) => acc + w, 0);
      expect(total).toBeLessThanOrEqual(400);
      // Compression respects the per-column floor.
      for (const width of widths) {
        expect(width).toBeGreaterThanOrEqual(MIN_TEXT_PX);
      }
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it('keeps ideal widths when the container width is unknown', () => {
    // Without a ResizeObserver (jsdom default) the table renders ideal
    // widths — the pre-fit behavior.
    const block: ReportBlockDefinition = {
      id: 'records',
      type: 'table',
      source: { schema: 'WorkflowButtonDemoItem', mode: 'filter' },
      table: {
        columns: [{ field: 'a', label: 'A' }],
      },
    };
    const result: ReportBlockResult = {
      type: 'table',
      status: 'ready',
      data: {
        columns: [{ key: 'a', label: 'A' }],
        rows: [
          {
            a: 'A long descriptive sentence that would push this column to its cap',
          },
        ],
      },
    };

    const { container } = renderTableBlock(block, result);
    const cols = container.querySelectorAll('col');
    expect(colWidthPx(cols[0])).toBe(MAX_TEXT_PX);
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
