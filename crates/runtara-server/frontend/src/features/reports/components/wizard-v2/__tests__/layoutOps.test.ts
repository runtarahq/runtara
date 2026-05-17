import { describe, expect, it } from 'vitest';
import {
  addBlock,
  collectLayoutBlockIds,
  moveBlock,
  orderedBlocksFromDefinition,
  removeBlock,
  removeBlockFromLayout,
  reorderLayoutBlocks,
  updateBlock,
} from '../layoutOps';
import {
  ReportBlockDefinition,
  ReportDefinition,
  ReportLayoutNode,
} from '../../../types';

function baseDefinition(): ReportDefinition {
  return {
    definitionVersion: 1,
    layout: [
      { id: 'n_a', type: 'block', blockId: 'a' },
      {
        id: 'n_section',
        type: 'section',
        title: 'Section',
        children: [
          { id: 'n_b', type: 'block', blockId: 'b' },
          {
            id: 'n_columns',
            type: 'columns',
            columns: [
              {
                id: 'n_col1',
                children: [{ id: 'n_c', type: 'block', blockId: 'c' }],
              },
            ],
          },
        ],
      },
    ],
    filters: [],
    blocks: [
      { id: 'a', type: 'markdown', source: { schema: '' } },
      { id: 'b', type: 'markdown', source: { schema: '' } },
      { id: 'c', type: 'markdown', source: { schema: '' } },
    ],
  };
}

describe('layoutOps', () => {
  it('collects block ids depth-first', () => {
    const ids = collectLayoutBlockIds(baseDefinition().layout);
    expect(ids).toEqual(['a', 'b', 'c']);
  });

  it('orders blocks by layout occurrence', () => {
    const ordered = orderedBlocksFromDefinition(baseDefinition());
    expect(ordered.map((b) => b.id)).toEqual(['a', 'b', 'c']);
  });

  it('appends unplaced blocks after layout-ordered ones', () => {
    const def = baseDefinition();
    def.blocks.push({ id: 'd', type: 'markdown', source: { schema: '' } });
    expect(orderedBlocksFromDefinition(def).map((b) => b.id)).toEqual([
      'a',
      'b',
      'c',
      'd',
    ]);
  });

  it('removes a block from both layout and blocks', () => {
    const next = removeBlock(baseDefinition(), 'b');
    expect(next.blocks.map((b) => b.id)).toEqual(['a', 'c']);
    expect(collectLayoutBlockIds(next.layout)).toEqual(['a', 'c']);
  });

  it('keeps wrapping structure when removing a deeply nested block', () => {
    const next = removeBlock(baseDefinition(), 'c');
    const section = next.layout?.find(
      (n): n is Extract<ReportLayoutNode, { type: 'section' }> =>
        n.type === 'section'
    );
    expect(section).toBeDefined();
    expect(section!.children?.some((n) => n.type === 'columns')).toBe(true);
  });

  it('adds a block at the top level of the layout', () => {
    const block: ReportBlockDefinition = {
      id: 'd',
      type: 'markdown',
      source: { schema: '' },
    };
    const next = addBlock(baseDefinition(), block);
    expect(next.blocks.map((b) => b.id)).toEqual(['a', 'b', 'c', 'd']);
    expect(collectLayoutBlockIds(next.layout)).toEqual(['a', 'b', 'c', 'd']);
  });

  it('moves a block forward and back', () => {
    const def = baseDefinition();
    const moved = moveBlock(def, 'a', 2);
    expect(collectLayoutBlockIds(moved.layout)).toEqual(['b', 'c', 'a']);
    const back = moveBlock(moved, 'a', 0);
    expect(collectLayoutBlockIds(back.layout)).toEqual(['a', 'b', 'c']);
  });

  it('updates a single block in place', () => {
    const next = updateBlock(baseDefinition(), 'b', (block) => ({
      ...block,
      title: 'Renamed',
    }));
    expect(next.blocks.find((b) => b.id === 'b')?.title).toBe('Renamed');
    // Layout untouched.
    expect(collectLayoutBlockIds(next.layout)).toEqual(['a', 'b', 'c']);
  });

  it('preserves unknown layout-node fields on remove', () => {
    const def = baseDefinition();
    // Add an opaque-but-typed `showWhen` on the section to check pass-through.
    const section = def.layout![1] as Extract<
      ReportLayoutNode,
      { type: 'section' }
    >;
    (section as unknown as Record<string, unknown>).showWhen = { filter: 'x' };
    const next = removeBlock(def, 'b');
    const surviving = next.layout![1] as Extract<
      ReportLayoutNode,
      { type: 'section' }
    >;
    expect(
      (surviving as unknown as Record<string, unknown>).showWhen
    ).toEqual({ filter: 'x' });
  });

  it('removeBlockFromLayout no-ops on missing id', () => {
    const def = baseDefinition();
    const next = removeBlockFromLayout(def.layout, 'missing');
    expect(next).toEqual(def.layout);
  });

  it('reorderLayoutBlocks handles set differences by drop+append', () => {
    const def = baseDefinition();
    // Drop 'b' and add 'd' via a set difference; verify layout matches.
    const next = reorderLayoutBlocks(def.layout, ['a', 'c', 'd']);
    expect(collectLayoutBlockIds(next)).toEqual(['a', 'c', 'd']);
  });
});
