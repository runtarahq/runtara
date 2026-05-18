import { describe, expect, it } from 'vitest';
import { resolveDrop } from '../dndResolve';
import { moveLayoutNode } from '../layoutOps';
import {
  ReportDefinition,
  ReportGridLayoutNode,
  ReportLayoutNode,
} from '../../../types';

/** Root grid containing three sibling blocks. */
function definitionWithGrid(): ReportDefinition {
  return {
    definitionVersion: 1,
    filters: [],
    blocks: [
      { id: 'a', type: 'markdown', source: { schema: '' } },
      { id: 'b', type: 'markdown', source: { schema: '' } },
      { id: 'c', type: 'markdown', source: { schema: '' } },
    ],
    layout: {
      id: 'g_root',
      columns: 1,
      items: [
        { id: 'item_a', child: { id: 'n_a', type: 'block', blockId: 'a' } },
        { id: 'item_b', child: { id: 'n_b', type: 'block', blockId: 'b' } },
        { id: 'item_c', child: { id: 'n_c', type: 'block', blockId: 'c' } },
      ],
    },
  };
}

/** Root grid with two nested sub-grids, each containing one block. */
function twoGridsDefinition(): ReportDefinition {
  return {
    definitionVersion: 1,
    filters: [],
    blocks: [
      { id: 'a', type: 'markdown', source: { schema: '' } },
      { id: 'b', type: 'markdown', source: { schema: '' } },
    ],
    layout: {
      id: 'root',
      columns: 1,
      items: [
        {
          id: 'item_g1',
          child: {
            id: 'g1',
            type: 'grid',
            columns: 1,
            items: [
              {
                id: 'item_a',
                child: { id: 'n_a', type: 'block', blockId: 'a' },
              },
            ],
          } as ReportLayoutNode,
        },
        {
          id: 'item_g2',
          child: {
            id: 'g2',
            type: 'grid',
            columns: 1,
            items: [
              {
                id: 'item_b',
                child: { id: 'n_b', type: 'block', blockId: 'b' },
              },
            ],
          } as ReportLayoutNode,
        },
      ],
    },
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

  it("drops onto a later sibling → lands at over's original index", () => {
    const def = definitionWithGrid();
    const res = resolveDrop(def, { sourceId: 'n_a', overId: 'n_c' });
    expect(res).toEqual({
      apply: true,
      target: { parentGridId: 'g_root', index: 2 },
    });
  });

  it('drops onto a nested-grid container → appends into that grid', () => {
    const def = twoGridsDefinition();
    const res = resolveDrop(def, { sourceId: 'n_a', overId: 'g2' });
    expect(res).toEqual({
      apply: true,
      target: { parentGridId: 'g2' },
    });
  });

  it('drops onto a sibling in a different nested grid → lands at that sibling index', () => {
    const def = twoGridsDefinition();
    const res = resolveDrop(def, { sourceId: 'n_a', overId: 'n_b' });
    expect(res).toEqual({
      apply: true,
      target: { parentGridId: 'g2', index: 0 },
    });
  });

  it('drops a nested grid onto another nested-grid container', () => {
    const def = twoGridsDefinition();
    const res = resolveDrop(def, { sourceId: 'g2', overId: 'g1' });
    expect(res).toEqual({
      apply: true,
      target: { parentGridId: 'g1' },
    });
  });

  it('drops onto the root grid (container drop)', () => {
    const def = twoGridsDefinition();
    const res = resolveDrop(def, { sourceId: 'n_a', overId: 'root' });
    expect(res).toEqual({
      apply: true,
      target: { parentGridId: 'root' },
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
    const order = next.layout.items.map((item) => item.child.id);
    expect(order).toEqual(['n_c', 'n_a', 'n_b']);
  });

  it('moving n_a onto n_c reorders to [b, c, a]', () => {
    const def = definitionWithGrid();
    const res = resolveDrop(def, { sourceId: 'n_a', overId: 'n_c' });
    if (!res.apply) throw new Error('expected apply');
    const next = moveLayoutNode(def, 'n_a', res.target);
    const order = next.layout.items.map((item) => item.child.id);
    expect(order).toEqual(['n_b', 'n_c', 'n_a']);
  });

  it('moving n_a onto g2 (nested container) moves it into the second grid', () => {
    const def = twoGridsDefinition();
    const res = resolveDrop(def, { sourceId: 'n_a', overId: 'g2' });
    if (!res.apply) throw new Error('expected apply');
    const next = moveLayoutNode(def, 'n_a', res.target);
    const g1 = next.layout.items[0].child as ReportGridLayoutNode;
    const g2 = next.layout.items[1].child as ReportGridLayoutNode;
    expect(g1.items).toHaveLength(0);
    expect(g2.items.map((item) => item.child.id)).toEqual(['n_b', 'n_a']);
  });

  it('moving n_a onto n_b (cross-grid sibling) places it before n_b in g2', () => {
    const def = twoGridsDefinition();
    const res = resolveDrop(def, { sourceId: 'n_a', overId: 'n_b' });
    if (!res.apply) throw new Error('expected apply');
    const next = moveLayoutNode(def, 'n_a', res.target);
    const g2 = next.layout.items[1].child as ReportGridLayoutNode;
    expect(g2.items.map((item) => item.child.id)).toEqual(['n_a', 'n_b']);
  });
});
