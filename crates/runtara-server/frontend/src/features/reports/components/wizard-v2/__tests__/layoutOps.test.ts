import { describe, expect, it } from 'vitest';
import {
  ROOT_GRID_ID,
  addBlock,
  addLayoutNode,
  collectLayoutBlockIds,
  makeBlockId,
  makeGridId,
  moveLayoutNode,
  newDefaultLayout,
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

/**
 * The root grid carries:
 *   root
 *     ├── block(n_a → "a")
 *     └── grid(g_outer, "Section", columns=1)
 *           ├── block(n_b → "b")
 *           └── grid(g_inner, columns=1)
 *                 └── block(n_c → "c")
 */
function baseDefinition(): ReportDefinition {
  return {
    definitionVersion: 1,
    layout: {
      id: 'root',
      columns: 1,
      items: [
        {
          id: 'root_i0',
          child: { id: 'n_a', type: 'block', blockId: 'a' },
        },
        {
          id: 'root_i1',
          child: {
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
        },
      ],
    },
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

  it('walkLayout visits every node under the root grid (root itself excluded)', () => {
    const visited: string[] = [];
    walkLayout(baseDefinition().layout, (node) => visited.push(node.id));
    expect(visited).toEqual(['n_a', 'g_outer', 'n_b', 'g_inner', 'n_c']);
  });
});

describe('defaults', () => {
  it('newDefaultLayout returns an empty 1x1 root grid', () => {
    const root = newDefaultLayout();
    expect(root.id).toBe(ROOT_GRID_ID);
    expect(root.columns).toBe(1);
    expect(root.rows).toBe(1);
    expect(root.items).toEqual([]);
  });
});

describe('block-side operations', () => {
  it('removeBlock strips block + every layout item referencing it', () => {
    const next = removeBlock(baseDefinition(), 'b');
    expect(next.blocks.map((b) => b.id)).toEqual(['a', 'c']);
    const ids: string[] = [];
    walkLayout(next.layout, (node) => ids.push(node.id));
    // n_b removed, but the outer grid + g_inner remain.
    expect(ids).toEqual(['n_a', 'g_outer', 'g_inner', 'n_c']);
  });

  it('addBlock appends item to the root grid by default', () => {
    const block: ReportBlockDefinition = {
      id: 'd',
      type: 'markdown',
      source: { schema: '' },
    };
    const next = addBlock(baseDefinition(), block);
    expect(next.blocks.map((b) => b.id)).toEqual(['a', 'b', 'c', 'd']);
    const lastItem =
      next.layout.items[next.layout.items.length - 1];
    expect(lastItem.child.type).toBe('block');
    if (lastItem.child.type === 'block') {
      expect(lastItem.child.blockId).toBe('d');
    }
  });

  it('updateBlock patches a block in-place without touching layout', () => {
    const before = baseDefinition();
    const next = updateBlock(before, 'b', (block) => ({
      ...block,
      title: 'Renamed',
    }));
    expect(next.blocks.find((b) => b.id === 'b')?.title).toBe('Renamed');
    expect(JSON.stringify(next.layout)).toBe(JSON.stringify(before.layout));
  });
});

describe('grid (layout-node) operations', () => {
  it('pathToLayoutNode returns the root for the root grid id', () => {
    const path = pathToLayoutNode(baseDefinition(), 'root');
    expect(path).toEqual({ parentGridId: null, itemIndex: null });
  });

  it('pathToLayoutNode returns parentGridId + itemIndex for a top-level child', () => {
    const path = pathToLayoutNode(baseDefinition(), 'n_a');
    expect(path).toEqual({ parentGridId: 'root', itemIndex: 0 });
  });

  it('pathToLayoutNode finds a node nested inside multiple grids', () => {
    const path = pathToLayoutNode(baseDefinition(), 'n_c');
    expect(path).toEqual({ parentGridId: 'g_inner', itemIndex: 0 });
  });

  it('pathToLayoutNode returns null when missing', () => {
    expect(pathToLayoutNode(baseDefinition(), 'missing')).toBe(null);
  });

  it('addLayoutNode with null parent appends into the root grid', () => {
    const newBlock: ReportLayoutNode = {
      id: 'n_d',
      type: 'block',
      blockId: 'd',
    };
    const next = addLayoutNode(baseDefinition(), newBlock, {
      parentGridId: null,
    });
    const lastItem =
      next.layout.items[next.layout.items.length - 1];
    expect(lastItem.child).toEqual(newBlock);
  });

  it('addLayoutNode into a nested grid wraps the node in a grid item', () => {
    const newBlock: ReportLayoutNode = {
      id: 'n_d',
      type: 'block',
      blockId: 'd',
    };
    const next = addLayoutNode(baseDefinition(), newBlock, {
      parentGridId: 'g_outer',
    });
    const outer = next.layout.items[1].child as ReportGridLayoutNode;
    expect(outer.items[outer.items.length - 1].child).toEqual(newBlock);
  });

  it('addLayoutNode into deeply nested grid finds the right container', () => {
    const newBlock: ReportLayoutNode = {
      id: 'n_d',
      type: 'block',
      blockId: 'd',
    };
    const next = addLayoutNode(baseDefinition(), newBlock, {
      parentGridId: 'g_inner',
    });
    const outer = next.layout.items[1].child as ReportGridLayoutNode;
    const inner = outer.items[1].child as ReportGridLayoutNode;
    expect(inner.items[inner.items.length - 1].child).toEqual(newBlock);
  });

  it('removeLayoutNode strips a node by id (both top-level and nested)', () => {
    const after = removeLayoutNode(baseDefinition(), 'g_inner');
    const ids: string[] = [];
    walkLayout(after.layout, (node) => ids.push(node.id));
    expect(ids).toEqual(['n_a', 'g_outer', 'n_b']);
  });

  it('removeLayoutNode is a no-op when targeting the root grid (root cannot be removed)', () => {
    const before = baseDefinition();
    const after = removeLayoutNode(before, before.layout.id);
    expect(JSON.stringify(after.layout)).toBe(JSON.stringify(before.layout));
  });

  it('moveLayoutNode is a no-op when targeting the root grid', () => {
    const before = baseDefinition();
    const after = moveLayoutNode(before, before.layout.id, {
      parentGridId: null,
    });
    expect(JSON.stringify(after.layout)).toBe(JSON.stringify(before.layout));
  });

  it('moveLayoutNode moves a nested node to a different grid', () => {
    const next = moveLayoutNode(baseDefinition(), 'n_b', {
      parentGridId: 'g_inner',
    });
    const outer = next.layout.items[1].child as ReportGridLayoutNode;
    // g_outer no longer holds n_b — only g_inner remains.
    expect(outer.items.length).toBe(1);
    expect(outer.items[0].child.id).toBe('g_inner');
    const inner = outer.items[0].child as ReportGridLayoutNode;
    expect(inner.items[inner.items.length - 1].child.id).toBe('n_b');
  });

  it('updateGrid patches the root grid metadata', () => {
    const next = updateGrid(baseDefinition(), 'root', (g) => ({
      ...g,
      title: 'Dashboard',
      columns: 3,
    }));
    expect(next.layout.title).toBe('Dashboard');
    expect(next.layout.columns).toBe(3);
    expect(next.layout.items.length).toBe(2);
  });

  it('updateGrid patches a nested grid in-place', () => {
    const next = updateGrid(baseDefinition(), 'g_outer', (g) => ({
      ...g,
      title: 'Renamed',
      columns: 2,
    }));
    const grid = next.layout.items[1].child as ReportGridLayoutNode;
    expect(grid.title).toBe('Renamed');
    expect(grid.columns).toBe(2);
    expect(grid.items.length).toBe(2);
  });

  it('updateGridItem patches a single item (e.g. colSpan)', () => {
    const next = updateGridItem(baseDefinition(), 'g_outer_i0', (item) => ({
      ...item,
      colSpan: 3,
    }));
    const grid = next.layout.items[1].child as ReportGridLayoutNode;
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
