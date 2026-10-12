import { describe, expect, it } from 'vitest';
import type { Node } from '@xyflow/react';
import { NODE_TYPES } from '@/features/workflows/config/workflow';
import {
  findAttachTarget,
  offsetNextToStep,
  placementForDrop,
  resolveNotePositions,
  stepRects,
} from './note-placement';

const step = (
  id: string,
  x: number,
  y: number,
  extra: Partial<Node> = {}
): Node => ({
  id,
  type: NODE_TYPES.BasicNode,
  position: { x, y },
  data: {},
  measured: { width: 216, height: 60 },
  ...extra,
});

const note = (
  id: string,
  x: number,
  y: number,
  anchorStepId?: string
): Node => ({
  id,
  type: NODE_TYPES.NoteNode,
  position: { x, y },
  data: { id, content: '', ...(anchorStepId ? { anchorStepId } : {}) },
  width: 240,
  height: 120,
});

describe('resolveNotePositions', () => {
  it('places an anchored note at its step plus the stored offset', () => {
    const nodes = [step('fetch', 300, 400), note('n1', 10, -144, 'fetch')];
    const resolved = resolveNotePositions(nodes);
    expect(resolved[1].position).toEqual({ x: 310, y: 256 });
  });

  it('follows the step when the layout moves it', () => {
    const before = resolveNotePositions([
      step('fetch', 300, 400),
      note('n1', 0, -144, 'fetch'),
    ]);
    const after = resolveNotePositions([
      step('fetch', 600, 400),
      note('n1', 0, -144, 'fetch'),
    ]);
    expect(after[1].position.x - before[1].position.x).toBe(300);
  });

  it('resolves a step inside a container to its absolute position', () => {
    const nodes = [
      step('loop', 100, 100, { type: NODE_TYPES.ContainerNode }),
      step('tick', 40, 60, { parentId: 'loop' }),
      note('n1', 0, -144, 'tick'),
    ];
    expect(resolveNotePositions(nodes)[2].position).toEqual({ x: 140, y: 16 });
  });

  it('leaves free notes and notes of missing steps where they are', () => {
    const nodes = [note('free', 50, 60), note('orphan', 7, 8, 'gone')];
    const resolved = resolveNotePositions(nodes);
    expect(resolved[0].position).toEqual({ x: 50, y: 60 });
    expect(resolved[1].position).toEqual({ x: 7, y: 8 });
  });
});

describe('findAttachTarget', () => {
  const steps = stepRects([step('a', 0, 0), step('b', 600, 0)]);

  it('attaches to the nearest step within range', () => {
    expect(
      findAttachTarget({ x: 0, y: -150, width: 240, height: 120 }, steps)
    ).toBe('a');
    expect(
      findAttachTarget({ x: 560, y: 80, width: 240, height: 120 }, steps)
    ).toBe('b');
  });

  it('keeps the current step while it is still in range', () => {
    // 80px from a and 64px from b: both in range, b closer.
    const nudged = { x: 296, y: 0, width: 240, height: 120 };
    expect(findAttachTarget(nudged, steps)).toBe('b');
    expect(findAttachTarget(nudged, steps, undefined, 'a')).toBe('a');
  });

  it('leaves a note dropped in open space free', () => {
    expect(
      findAttachTarget({ x: 0, y: -600, width: 240, height: 120 }, steps)
    ).toBeNull();
  });
});

describe('placementForDrop', () => {
  const steps = stepRects([step('a', 300, 400)]);

  it('stores an offset from the anchor step', () => {
    expect(placementForDrop({ x: 320, y: 250 }, 'a', steps)).toEqual({
      position: { x: 20, y: -150 },
      anchorStepId: 'a',
    });
  });

  it('stores absolute coordinates for a free note', () => {
    expect(placementForDrop({ x: 320, y: 250 }, null, steps)).toEqual({
      position: { x: 320, y: 250 },
      anchorStepId: null,
    });
  });

  it('round-trips through resolveNotePositions', () => {
    const placement = placementForDrop({ x: 320, y: 250 }, 'a', steps);
    const [, resolved] = resolveNotePositions([
      step('a', 300, 400),
      note('n', placement.position.x, placement.position.y, 'a'),
    ]);
    expect(resolved.position).toEqual({ x: 320, y: 250 });
  });
});

describe('offsetNextToStep', () => {
  it('puts a new note just above the step', () => {
    expect(offsetNextToStep(120)).toEqual({ x: 0, y: -144 });
  });
});
