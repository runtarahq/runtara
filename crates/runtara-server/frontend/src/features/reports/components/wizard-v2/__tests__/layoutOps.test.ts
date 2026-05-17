import { describe, expect, it } from 'vitest';
import {
  addBlock,
  addLayoutNode,
  collectLayoutBlockIds,
  makeBlockId,
  makeGridId,
  moveBlock,
  moveLayoutNode,
  newGrid,
  orderedBlocksFromDefinition,
  pathToLayoutNode,
  removeBlock,
  removeLayoutNode,
  updateBlock,
  updateGrid,
  updateGridItem,
  walkLayout,
} from '../layoutOps';
import {
  ReportBlockDefinition,
  ReportDefinition,
  ReportGridLayoutNode,
  ReportLayoutNode,
} from '../../../types';

function baseDefinition(): ReportDefinition {
  return {
    definitionVersion: 1,
    layout: [
      { id: 'n_a', type: 'block', blockId: 'a' },
      {
        id: 'g_outer',
        type: 'grid',
        title: 'Section',
        columns: 1,
        items: [
          {
            id: 'g_outer_i0',
            child: { id: 'n_b', type: 'block', blockId: 'b' },
          },
          {
            id: 'g_outer_i1',
            child: {
              id: 'g_inner',
              type: 'grid',
              columns: 1,
              items: [
                {
                  id: 'g_inner_i0',
                  child: { id: 'n_c', type: 'block', blockId: 'c' },
                },
              ],
            } as ReportLayoutNode,
          },
        ],
      } as ReportLayoutNode,
    ],
    filters: [],
    blocks: [
      { id: 'a', type: 'markdown', source: { schema: '' } },
      { id: 'b', type: 'markdown', source: { schema: '' } },
      { id: 'c', type: 'markdown', source: { schema: '' } },
    ],
  };
}

describe('layoutOps walkers', () => {
  it('collectLayoutBlockIds depth-first across nested grids', () => {
    expect(collectLayoutBlockIds(baseDefinition().layout)).toEqual([
      'a',
      'b',
      'c',
    ]);
  });

  it('orderedBlocksFromDefinition reflects layout order', () => {
    const ordered = orderedBlocksFromDefinition(baseDefinition());
    expect(ordered.map((b) => b.id)).toEqual(['a', 'b', 'c']);
  });

  it('orderedBlocksFromDefinition appends unplaced blocks', () => {
    const def = baseDefinition();
    def.blocks.push({ id: 'd', type: 'markdown', source: { schema: '' } });
    expect(orderedBlocksFromDefinition(def).map((b) => b.id)).toEqual([
      'a',
      'b',
      'c',
      'd',
    ]);
  });

  it('walkLayout visits every grid item child', () => {
    const visited: string[] = [];
    walkLayout(baseDefinition().layout, (node) => visited.push(node.id));
    expect(visited).toEqual([
      'n_a',
      'g_outer',
      'n_b',
      'g_inner',
      'n_c',
    ]);
  });
});

describe('block-side operations', () => {
  it('removeBlock strips block + matching layout entries (top-level + grid items)', () => {
    const next = removeBlock(baseDefinition(), 'b');
    expect(next.blocks.map((b) => b.id)).toEqual(['a', 'c']);
    const ids: string[] = [];
    walkLayout(next.layout, (node) => ids.push(node.id));
    // n_b removed, but the outer grid + g_inner remain.
    expect(ids).toEqual(['n_a', 'g_outer', 'g_inner', 'n_c']);
  });

  it('addBlock appends top-level layout entry', () => {
    const block: ReportBlockDefinition = {
      id: 'd',
      type: 'markdown',
      source: { schema: '' },
    };
    const next = addBlock(baseDefinition(), block);
    expect(next.blocks.map((b) => b.id)).toEqual(['a', 'b', 'c', 'd']);
    const last = next.layout?.[next.layout.length - 1];
    expect(last?.type).toBe('block');
    if (last?.type === 'block') expect(last.blockId).toBe('d');
  });

  it('updateBlock patches a block in-place', () => {
    const next = updateBlock(baseDefinition(), 'b', (block) => ({
      ...block,
      title: 'Renamed',
    }));
    expect(next.blocks.find((b) => b.id === 'b')?.title).toBe('Renamed');
    expect(JSON.stringify(next.layout)).toBe(
      JSON.stringify(baseDefinition().layout)
    );
  });

  it('moveBlock reorders top-level block siblings without touching grids', () => {
    // The only top-level block is `a`; moveBlock here is a no-op.
    const next = moveBlock(baseDefinition(), 'a', 2);
    expect(JSON.stringify(next.layout)).toBe(
      JSON.stringify(baseDefinition().layout)
    );
  });
});

describe('grid (layout-node) operations', () => {
  it('pathToLayoutNode returns root index for top-level node', () => {
    const path = pathToLayoutNode(baseDefinition(), 'n_a');
    expect(path).toEqual({
      parentGridId: null,
      itemIndex: null,
      rootIndex: 0,
    });
  });

  it('pathToLayoutNode returns parentGridId + itemIndex for nested node', () => {
    const path = pathToLayoutNode(baseDefinition(), 'n_b');
    expect(path).toEqual({
      parentGridId: 'g_outer',
      itemIndex: 0,
      rootIndex: null,
    });
  });

  it('pathToLayoutNode finds deeply nested grid', () => {
    const path = pathToLayoutNode(baseDefinition(), 'n_c');
    expect(path).toEqual({
      parentGridId: 'g_inner',
      itemIndex: 0,
      rootIndex: null,
    });
  });

  it('pathToLayoutNode returns null when missing', () => {
    expect(pathToLayoutNode(baseDefinition(), 'missing')).toBe(null);
  });

  it('addLayoutNode at root appends a top-level layout node', () => {
    const newBlock: ReportLayoutNode = {
      id: 'n_d',
      type: 'block',
      blockId: 'd',
    };
    const next = addLayoutNode(baseDefinition(), newBlock, {
      parentGridId: null,
    });
    expect(next.layout?.[next.layout.length - 1]).toEqual(newBlock);
  });

  it('addLayoutNode into a grid wraps node in a grid item', () => {
    const newBlock: ReportLayoutNode = {
      id: 'n_d',
      type: 'block',
      blockId: 'd',
    };
    const next = addLayoutNode(baseDefinition(), newBlock, {
      parentGridId: 'g_outer',
    });
    const grid = next.layout?.[1];
    expect(grid?.type).toBe('grid');
    if (grid?.type === 'grid') {
      expect(grid.items[grid.items.length - 1].child).toEqual(newBlock);
    }
  });

  it('addLayoutNode into nested grid finds the right container', () => {
    const newBlock: ReportLayoutNode = {
      id: 'n_d',
      type: 'block',
      blockId: 'd',
    };
    const next = addLayoutNode(baseDefinition(), newBlock, {
      parentGridId: 'g_inner',
    });
    const outer = next.layout?.[1];
    expect(outer?.type).toBe('grid');
    if (outer?.type === 'grid') {
      const inner = outer.items[1].child;
      expect(inner.type).toBe('grid');
      if (inner.type === 'grid') {
        expect(inner.items[1].child).toEqual(newBlock);
      }
    }
  });

  it('removeLayoutNode strips a node by id (root + nested)', () => {
    const after = removeLayoutNode(baseDefinition(), 'g_inner');
    const ids: string[] = [];
    walkLayout(after.layout, (node) => ids.push(node.id));
    expect(ids).toEqual(['n_a', 'g_outer', 'n_b']);
  });

  it('moveLayoutNode moves a nested node to a different grid', () => {
    const next = moveLayoutNode(baseDefinition(), 'n_b', {
      parentGridId: 'g_inner',
    });
    const outer = next.layout?.[1];
    expect(outer?.type).toBe('grid');
    if (outer?.type === 'grid') {
      // After moving, g_outer should only contain the inner grid (n_b
      // was removed from it).
      expect(outer.items.length).toBe(1);
      expect(outer.items[0].child.id).toBe('g_inner');
      const inner = outer.items[0].child;
      if (inner.type === 'grid') {
        // n_b appended to inner.
        const lastChildId = inner.items[inner.items.length - 1].child.id;
        expect(lastChildId).toBe('n_b');
      }
    }
  });

  it('updateGrid patches grid metadata in-place', () => {
    const next = updateGrid(baseDefinition(), 'g_outer', (g) => ({
      ...g,
      title: 'Renamed',
      columns: 2,
    }));
    const grid = next.layout?.[1] as ReportGridLayoutNode;
    expect(grid.title).toBe('Renamed');
    expect(grid.columns).toBe(2);
    // Items untouched.
    expect(grid.items.length).toBe(2);
  });

  it('updateGridItem patches a single item (e.g. colSpan)', () => {
    const next = updateGridItem(baseDefinition(), 'g_outer_i0', (item) => ({
      ...item,
      colSpan: 3,
    }));
    const grid = next.layout?.[1] as ReportGridLayoutNode;
    expect(grid.items[0].colSpan).toBe(3);
    expect(grid.items[1].colSpan).toBeUndefined();
  });
});

describe('id helpers', () => {
  it('makeBlockId derives from seed', () => {
    expect(makeBlockId('Hello World')).toMatch(/^hello_world_/);
  });

  it('makeBlockId falls back when seed is empty', () => {
    expect(makeBlockId('')).toMatch(/^block_/);
  });

  it('makeGridId prefixes grid_', () => {
    expect(makeGridId()).toMatch(/^grid_/);
  });

  it('newGrid materializes a fresh grid with the supplied preset', () => {
    const grid = newGrid({
      columns: 2,
      title: 'Two cols',
    });
    expect(grid.type).toBe('grid');
    if (grid.type === 'grid') {
      expect(grid.columns).toBe(2);
      expect(grid.title).toBe('Two cols');
      expect(grid.items).toEqual([]);
    }
  });
});
