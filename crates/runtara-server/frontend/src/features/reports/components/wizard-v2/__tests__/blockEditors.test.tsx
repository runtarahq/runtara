import { describe, expect, it, vi } from 'vitest';
import { render, fireEvent } from '@testing-library/react';
import { MarkdownBlockEditor } from '../blocks/MarkdownBlockEditor';
import { TableBlockEditor } from '../blocks/TableBlockEditor';
import { ReportBlockDefinition } from '../../../types';
import { Schema } from '@/generated/RuntaraRuntimeApi';

describe('MarkdownBlockEditor', () => {
  it('emits a new block when the textarea changes', () => {
    const block: ReportBlockDefinition = {
      id: 'intro',
      type: 'markdown',
      source: { schema: '' },
      markdown: { content: '# Hello' },
    };
    const onChange = vi.fn();
    const { getByLabelText } = render(
      <MarkdownBlockEditor block={block} onChange={onChange} />
    );
    fireEvent.change(getByLabelText(/markdown content/i), {
      target: { value: '# Updated' },
    });
    expect(onChange).toHaveBeenCalledTimes(1);
    const next = onChange.mock.calls[0][0] as ReportBlockDefinition;
    expect(next.markdown?.content).toBe('# Updated');
    expect(next.id).toBe('intro');
  });

  it('preserves unknown block-level fields', () => {
    const block = {
      id: 'intro',
      type: 'markdown' as const,
      source: { schema: '' },
      markdown: { content: '' },
      __unknown: { kept: true },
    } as ReportBlockDefinition;
    const onChange = vi.fn();
    const { getByLabelText } = render(
      <MarkdownBlockEditor block={block} onChange={onChange} />
    );
    fireEvent.change(getByLabelText(/markdown content/i), {
      target: { value: 'x' },
    });
    const next = onChange.mock.calls[0][0] as Record<string, unknown>;
    expect(next.__unknown).toEqual({ kept: true });
  });
});

describe('TableBlockEditor', () => {
  const ordersSchema = {
    id: 'schema_order',
    tenantId: 't1',
    tableName: 'orders',
    name: 'Order',
    columns: [
      { name: 'order_id', type: 'string' },
      { name: 'total_amount', type: 'number' },
    ],
    createdAt: new Date().toISOString(),
    updatedAt: new Date().toISOString(),
  } as unknown as Schema;

  it('renders columns without modifying them', () => {
    const block: ReportBlockDefinition = {
      id: 't',
      type: 'table',
      source: { schema: 'Order' },
      table: {
        columns: [{ field: 'order_id', label: 'Order' }],
      },
    };
    const onChange = vi.fn();
    const { getByText } = render(
      <TableBlockEditor
        block={block}
        schemas={[ordersSchema]}
        onChange={onChange}
      />
    );
    expect(getByText('order_id')).toBeTruthy();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('preserves existing column metadata when adding a new column', () => {
    const block: ReportBlockDefinition = {
      id: 't',
      type: 'table',
      source: { schema: 'Order' },
      table: {
        columns: [
          {
            field: 'order_id',
            label: 'Order',
            descriptive: true,
            displayTemplate: '{{ order_id }}',
          },
        ],
      },
    };
    const onChange = vi.fn();
    const { getByText } = render(
      <TableBlockEditor
        block={block}
        schemas={[ordersSchema]}
        onChange={onChange}
      />
    );
    fireEvent.click(getByText(/add column/i));
    const next = onChange.mock.calls[0][0] as ReportBlockDefinition;
    expect(next.table?.columns).toHaveLength(2);
    expect(next.table?.columns?.[0].descriptive).toBe(true);
    expect(next.table?.columns?.[0].displayTemplate).toBe('{{ order_id }}');
  });
});
