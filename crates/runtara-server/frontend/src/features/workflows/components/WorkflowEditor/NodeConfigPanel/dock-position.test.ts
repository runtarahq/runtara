import { describe, it, expect } from 'vitest';
import {
  canvasInset,
  connectorGeometry,
  DOCK_GAP,
  DOCK_MIN_WIDTH,
  isDocked,
  panelGeometry,
  type Rect,
} from './dock-position';

const rect = (top: number, height: number, left = 0, right = 100): Rect => ({
  top,
  bottom: top + height,
  left,
  right,
});

describe('isDocked', () => {
  it('docks at and above the breakpoint, sheets below it', () => {
    expect(isDocked({ width: DOCK_MIN_WIDTH, height: 900 })).toBe(true);
    expect(isDocked({ width: DOCK_MIN_WIDTH - 1, height: 900 })).toBe(false);
  });
});

describe('panelGeometry', () => {
  it('pins to the top and the right gutter', () => {
    const g = panelGeometry({ width: 1600, height: 1000 }, 520);
    expect(g.top).toBe(DOCK_GAP);
    expect(g.left).toBe(1600 - 520 - DOCK_GAP);
    expect(g.maxHeight).toBe(1000 - DOCK_GAP * 2);
  });

  it('does not move with viewport height — the whole point of pinning', () => {
    const short = panelGeometry({ width: 1600, height: 800 }, 520);
    const tall = panelGeometry({ width: 1600, height: 1600 }, 520);
    expect(short.top).toBe(tall.top);
    expect(short.left).toBe(tall.left);
  });

  it('never pushes the panel off the left edge on a narrow viewport', () => {
    const g = panelGeometry({ width: 400, height: 800 }, 520);
    expect(g.left).toBe(DOCK_GAP);
  });
});

describe('connectorGeometry', () => {
  const panel: Rect = { top: 16, bottom: 984, left: 1064, right: 1584 };

  it('lands on the node centre and spans the gap', () => {
    const node = rect(300, 56, 200, 900);
    const c = connectorGeometry(node, panel);
    expect(c.visible).toBe(true);
    expect(c.top).toBe(328); // 300 + 56/2
    expect(c.left).toBe(900);
    expect(c.width).toBe(164); // 1064 - 900
  });

  it('tracks the node rather than the panel', () => {
    const a = connectorGeometry(rect(200, 56, 200, 900), panel);
    const b = connectorGeometry(rect(600, 56, 200, 900), panel);
    expect(a.top).not.toBe(b.top);
    expect(b.top - a.top).toBe(400);
  });

  it('clamps into the panel span rather than pointing past it', () => {
    const high = connectorGeometry(rect(0, 20, 200, 900), panel);
    expect(high.top).toBe(panel.top + 12);
    const low = connectorGeometry(rect(970, 20, 200, 900), panel);
    expect(low.top).toBe(panel.bottom - 12);
  });

  it('hides when the node is fully outside the panel span', () => {
    expect(connectorGeometry(rect(-200, 56, 200, 900), panel).visible).toBe(
      false
    );
    expect(connectorGeometry(rect(1200, 56, 200, 900), panel).visible).toBe(
      false
    );
  });

  it('hides when the node sits underneath the panel', () => {
    // Nothing to span: a line of zero or negative length points at nothing.
    const under = connectorGeometry(rect(300, 56, 1100, 1400), panel);
    expect(under.visible).toBe(false);
    expect(under.width).toBe(0);
  });

  it('hides when there is no anchored node', () => {
    expect(connectorGeometry(null, panel).visible).toBe(false);
  });
});

describe('canvasInset', () => {
  it('reserves room for the panel when docked and open', () => {
    expect(canvasInset({ width: 1600, height: 1000 }, 520, true)).toBe(
      520 + DOCK_GAP * 2
    );
  });

  it('reserves nothing when closed', () => {
    expect(canvasInset({ width: 1600, height: 1000 }, 520, false)).toBe(0);
  });

  it('reserves nothing in sheet mode — the sheet overlays the bottom', () => {
    expect(canvasInset({ width: 900, height: 1000 }, 520, true)).toBe(0);
  });
});
