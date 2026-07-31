import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import type { Instance, Schema } from '@/generated/RuntaraRuntimeApi';
import { objectInstancesColumns } from './ObjectInstancesColumns';
import { DRAFT_ID_PREFIX, makeDraftInstance } from './draft-row';

function renderIdCell(instance: Instance) {
  const columns = objectInstancesColumns({
    objectSchemaDto: {
      id: 'schema-1',
      name: 'Orders',
      tableName: 'orders',
      tenantId: 'tenant-1',
      createdAt: '2026-07-27T00:00:00Z',
      updatedAt: '2026-07-27T00:00:00Z',
      columns: [],
    } satisfies Schema,
    onUpdate: vi.fn(),
    editingCellId: null,
    setEditingCellId: vi.fn(),
  });

  const idColumn = columns.find((column) => column.id === '_id')!;
  const cell = idColumn.cell as (props: {
    row: { original: Instance };
  }) => React.ReactElement;
  render(cell({ row: { original: instance } }));
}

describe('objectInstancesColumns', () => {
  it('omits generated tsvector columns from the instance grid', () => {
    const schema = {
      id: 'schema-1',
      name: 'CategoryTreeNode',
      tableName: 'category_tree_node',
      tenantId: 'tenant-1',
      createdAt: '2026-05-10T00:00:00Z',
      updatedAt: '2026-05-10T00:00:00Z',
      columns: [
        { name: 'name', type: 'string' },
        { name: 'search_blob', type: 'string' },
        {
          name: 'search_tsv',
          type: 'tsvector',
          sourceColumn: 'search_blob',
          language: 'english',
        },
      ],
    } satisfies Schema;

    const columns = objectInstancesColumns({
      objectSchemaDto: schema,
      onUpdate: vi.fn(),
      editingCellId: null,
      setEditingCellId: vi.fn(),
    });

    expect(columns.map((column) => column.id)).toContain('name');
    expect(columns.map((column) => column.id)).toContain('search_blob');
    expect(columns.map((column) => column.id)).not.toContain('search_tsv');
  });

  it('marks a draft row as Draft instead of offering to copy its placeholder id', () => {
    renderIdCell(makeDraftInstance(`${DRAFT_ID_PREFIX}1753600000000`));

    expect(screen.getByText('Draft')).toBeInTheDocument();
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
  });

  it('renders the copy-id button for a persisted row', () => {
    renderIdCell({
      id: '550e8400-e29b-41d4-a716-446655440000',
      tenantId: 'tenant-1',
      createdAt: '2026-07-27T00:00:00Z',
      updatedAt: '2026-07-27T00:00:00Z',
      properties: {},
    });

    expect(screen.getByRole('button')).toBeInTheDocument();
    expect(screen.queryByText('Draft')).not.toBeInTheDocument();
  });
});
