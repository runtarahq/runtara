import { describe, expect, it, vi } from 'vitest';
import type { Schema } from '@/generated/RuntaraRuntimeApi';
import { objectInstancesColumns } from './ObjectInstancesColumns';

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
});
