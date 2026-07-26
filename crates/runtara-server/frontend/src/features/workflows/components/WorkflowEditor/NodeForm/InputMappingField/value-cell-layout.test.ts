import { describe, it, expect } from 'vitest';
import {
  TOGGLE_GAP_PX,
  TOGGLE_GUTTER_CLASS,
  TOGGLE_SIZE_CLASS,
  TOGGLE_SIZE_PX,
  VALUE_CELL_CLASS,
} from './value-cell-layout';

/** Tailwind spacing unit: `size-9` / `mr-11` are multiples of 4px. */
const spacingPx = (cls: string, prefix: string): number => {
  const m = new RegExp(`${prefix}-(\\d+)`).exec(cls);
  if (!m) throw new Error(`no ${prefix}-N in ${JSON.stringify(cls)}`);
  return Number(m[1]) * 4;
};

const remPx = (cls: string): number => {
  const m = /calc\(100%-([\d.]+)rem\)/.exec(cls);
  if (!m) throw new Error(`no calc(100%-Nrem) in ${JSON.stringify(cls)}`);
  return Number(m[1]) * 16;
};

describe('value cell layout', () => {
  it('spells the toggle size the same way in px and in Tailwind', () => {
    expect(spacingPx(TOGGLE_SIZE_CLASS, 'size')).toBe(TOGGLE_SIZE_PX);
  });

  // The point of the whole module: a control with no toggle has to leave
  // exactly the room a toggle would have taken, or its row steps out of line.
  it('reserves exactly the toggle plus its gap', () => {
    const reserved = TOGGLE_SIZE_PX + TOGGLE_GAP_PX;
    expect(spacingPx(TOGGLE_GUTTER_CLASS, 'mr')).toBe(reserved);
    expect(remPx(TOGGLE_GUTTER_CLASS)).toBe(reserved);
  });

  it('overrides the padding rule rather than merely restating it', () => {
    // Without `!` the has-variant selector from TableCell wins and the
    // override is a no-op, which is invisible until a boolean field appears.
    expect(VALUE_CELL_CLASS).toContain('!pr-');
    expect(VALUE_CELL_CLASS).toContain('[role=checkbox]');
  });
});
