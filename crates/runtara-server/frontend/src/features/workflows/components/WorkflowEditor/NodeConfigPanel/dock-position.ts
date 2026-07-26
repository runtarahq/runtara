/**
 * Geometry for the docked step inspector.
 *
 * The panel is pinned: it occupies the right gutter at a fixed offset and does
 * not move when the selection changes. Only the connector moves.
 *
 * An earlier design slid the panel to top-align with the selected node. It was
 * worse in three ways: it aligned at tall viewports and hit the viewport clamp
 * at short ones (same product, different behaviour by window height), it
 * drifted on every canvas pan/scroll, and a full-height panel top-aligned to a
 * node lines up along a few percent of its edge anyway. The panel is the
 * surface you work in, so it stays put.
 */

export interface Rect {
  top: number;
  bottom: number;
  left: number;
  right: number;
}

export interface Viewport {
  width: number;
  height: number;
}

/** Docked width of the step inspector, in px. */
export const NODE_CONFIG_PANEL_WIDTH = 520;

/** Gap between the panel and the viewport edges, in px. */
export const DOCK_GAP = 16;

/** Below this width there is no gutter to dock into; the panel becomes a sheet. */
export const DOCK_MIN_WIDTH = 1080;

export function isDocked(viewport: Viewport): boolean {
  return viewport.width >= DOCK_MIN_WIDTH;
}

/** Fixed geometry of the panel itself. */
export function panelGeometry(
  viewport: Viewport,
  width: number
): { top: number; left: number; width: number; maxHeight: number } {
  return {
    top: DOCK_GAP,
    left: Math.max(DOCK_GAP, viewport.width - width - DOCK_GAP),
    width,
    maxHeight: Math.max(0, viewport.height - DOCK_GAP * 2),
  };
}

export interface Connector {
  /** Vertical position of the horizontal tie line. */
  top: number;
  left: number;
  width: number;
  /** Hidden when the node is not vertically within the panel's span. */
  visible: boolean;
}

/**
 * The tie line from the anchored node's right edge to the panel's left edge.
 *
 * Clamped into the panel's vertical span so it never points past the panel,
 * and hidden outright once the node has scrolled or panned out of that span —
 * a connector pointing at nothing is worse than no connector.
 */
export function connectorGeometry(node: Rect | null, panel: Rect): Connector {
  if (!node) return { top: 0, left: 0, width: 0, visible: false };

  const centre = node.top + (node.bottom - node.top) / 2;
  const withinPanelSpan = node.bottom > panel.top && node.top < panel.bottom;
  // A node to the right of the panel's left edge is underneath it: there is no
  // gap to span, so there is nothing to draw.
  const gap = panel.left - node.right;

  return {
    top: Math.min(Math.max(centre, panel.top + 12), panel.bottom - 12),
    left: node.right,
    width: Math.max(0, gap),
    visible: withinPanelSpan && gap > 2,
  };
}

/**
 * Right-hand inset the canvas needs so the panel does not cover it.
 * Zero when undocked — the sheet overlays the bottom instead.
 */
export function canvasInset(
  viewport: Viewport,
  width: number,
  open: boolean
): number {
  if (!open || !isDocked(viewport)) return 0;
  return width + DOCK_GAP * 2;
}
