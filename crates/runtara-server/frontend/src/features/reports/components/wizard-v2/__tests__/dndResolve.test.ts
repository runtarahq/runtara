import { describe, expect, it } from 'vitest';
import { resolveDrop } from '../dndResolve';
import { moveLayoutNode } from '../layoutOps';
import { ReportDefinition, ReportLayoutNode } from '../../../types';

function definitionWithGrid(): ReportDefinition {
  return {
    definitionVersion: 1,
    filters: [],
    blocks: [
      { id: 'a', type: 'markdown', source: { schema: '' } },
      { id: 'b', type: 'markdown', source: { schema: '' } },
      { id: 'c', type: 'markdown', source: { schema: '' } },
    ],
    layout: [
      {
        id: 'g_root',
        type: 'grid',
        columns: 1,
        items: [
          { id: 'item_a', child: { id: 'n_a', type: 'block', blockId: 'a' } },
          { id: 'item_b', child: { id: 'n_b', type: 'block', blockId: 'b' } },
          { id: 'item_c', child: { id: 'n_c', type: 'block', blockId: 'c' } },
        ],
      } as ReportLayoutNode,
    ],
  };
}

function twoGridsDefinition(): ReportDefinition {
  return {
    definitionVersion: 1,
    filters: [],
    blocks: [
      { id: 'a', type: 'markdown', source: { schema: '' } },
      { id: 'b', type: 'markdown', source: { schema: '' } },
    ],
    layout: [
      {
        id: 'g1',
        type: 'grid',
        columns: 1,
        items: [
          { id: 'item_a', child: { id: 'n_a', type: 'block', blockId: 'a' } },
        ],
      } as ReportLayoutNode,
      {
        id: 'g2',
        type: 'grid',
        columns: 1,
        items: [
          { id: 'item_b', child: { id: 'n_b', type: 'block', blockId: 'b' } },
        ],
      } as ReportLayoutNode,
    ],
  };
}

describe('resolveDrop', () => {
  it('no-ops when source === over', () => {
    const def = definitionWithGrid();
    expect(resolveDrop(def, { sourceId: 'n_a', overId: 'n_a' })).toEqual({
      apply: false,
    });
  });

  it('drops onto a sibling → lands at sibling index in same parent', () => {
    const def = definitionWithGrid();
    // Drop n_c onto n_a: n_c should land at index 0 in g_root.
    const res = resolveDrop(def, { sourceId: 'n_c', overId: 'n_a' });
    expect(res).toEqual({
      apply: true,
      target: { parentGridId: 'g_root', index: 0 },
    });
  });

  it('drops onto a later sibling in the same grid → lands at over\'s original index (arrayMove semantic)', () => {
    const def = definitionWithGrid();
    // Drag n_a (index 0) onto n_c (index 2). arrayMove semantic: result
    // [b, c, a]. moveLayoutNode is remove-then-add, so passing
    // index = overIndex = 2 means: after remove drops length to 2, the
    // insert at index 2 appends the source past c.
    const res = resolveDrop(def, { sourceId: 'n_a', overId: 'n_c' });
    expect(res).toEqual({
      apply: true,
      target: { parentGridId: 'g_root', index: 2 },
    });
  });

  it('drops onto a grid container → appends into that grid', () => {
    const def = twoGridsDefinition();
    const res = resolveDrop(def, { sourceId: 'n_a', overId: 'g2' });
    expect(res).toEqual({
      apply: true,
      target: { parentGridId: 'g2' },
    });
  });

  it('drops onto a sibling in a different grid → lands at that sibling index', () => {
    const def = twoGridsDefinition();
    const res = resolveDrop(def, { sourceId: 'n_a', overId: 'n_b' });
    expect(res).toEqual({
      apply: true,
      target: { parentGridId: 'g2', index: 0 },
    });
  });

  it('drops a grid onto another grid container → appends as a nested grid', () => {
    const def = twoGridsDefinition();
    const res = resolveDrop(def, { sourceId: 'g2', overId: 'g1' });
    expect(res).toEqual({
      apply: true,
      target: { parentGridId: 'g1' },
    });
  });

  it('no-ops when over-target id is missing', () => {
    const def = definitionWithGrid();
    expect(
      resolveDrop(def, { sourceId: 'n_a', overId: 'missing' })
    ).toEqual({ apply: false });
  });
});

describe('resolveDrop + moveLayoutNode end-to-end', () => {
  it('moving n_c onto n_a reorders to [c, a, b]', () => {
    const def = definitionWithGrid();
    const res = resolveDrop(def, { sourceId: 'n_c', overId: 'n_a' });
    if (!res.apply) throw new Error('expected apply');
    const next = moveLayoutNode(def, 'n_c', res.target);
    const grid = next.layout?.[0];
    if (grid?.type !== 'grid') throw new Error('expected grid');
    const order = grid.items.map((item) => item.child.id);
    expect(order).toEqual(['n_c', 'n_a', 'n_b']);
  });

  it('moving n_a onto n_c reorders to [b, c, a]', () => {
    const def = definitionWithGrid();
    const res = resolveDrop(def, { sourceId: 'n_a', overId: 'n_c' });
    if (!res.apply) throw new Error('expected apply');
    const next = moveLayoutNode(def, 'n_a', res.target);
    const grid = next.layout?.[0];
    if (grid?.type !== 'grid') throw new Error('expected grid');
    const order = grid.items.map((item) => item.child.id);
    expect(order).toEqual(['n_b', 'n_c', 'n_a']);
  });

  it('moving n_a onto g2 (container) moves it into the second grid', () => {
    const def = twoGridsDefinition();
    const res = resolveDrop(def, { sourceId: 'n_a', overId: 'g2' });
    if (!res.apply) throw new Error('expected apply');
    const next = moveLayoutNode(def, 'n_a', res.target);
    const g1 = next.layout?.[0];
    const g2 = next.layout?.[1];
    if (g1?.type !== 'grid' || g2?.type !== 'grid')
      throw new Error('expected two grids');
    expect(g1.items).toHaveLength(0);
    expect(g2.items.map((item) => item.child.id)).toEqual(['n_b', 'n_a']);
  });

  it('moving n_a onto n_b (cross-grid sibling) places it before n_b in g2', () => {
    const def = twoGridsDefinition();
    const res = resolveDrop(def, { sourceId: 'n_a', overId: 'n_b' });
    if (!res.apply) throw new Error('expected apply');
    const next = moveLayoutNode(def, 'n_a', res.target);
    const g2 = next.layout?.[1];
    if (g2?.type !== 'grid') throw new Error('expected g2');
    expect(g2.items.map((item) => item.child.id)).toEqual(['n_a', 'n_b']);
  });
});
