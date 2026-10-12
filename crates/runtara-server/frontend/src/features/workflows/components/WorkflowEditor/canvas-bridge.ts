import type { XYPosition } from '@xyflow/react';

/**
 * Screen → canvas coordinates for code outside the canvas.
 *
 * The editor wraps the canvas in its own ReactFlowProvider, so a
 * `useReactFlow()` higher up (the workflow page's toolbar) talks to a
 * different, never-rendered instance: its viewport is always the identity,
 * which is why toolbar-added notes landed at arbitrary spots. The canvas
 * registers its real mapping here while it is mounted.
 */
let screenToCanvasMapper: ((point: XYPosition) => XYPosition) | null = null;

export function registerScreenToCanvas(
  mapper: ((point: XYPosition) => XYPosition) | null
) {
  screenToCanvasMapper = mapper;
}

/** Canvas coordinates of a screen point, or null when no canvas is mounted. */
export function screenToCanvas(point: XYPosition): XYPosition | null {
  return screenToCanvasMapper ? screenToCanvasMapper(point) : null;
}
