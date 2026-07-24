/**
 * z-index ladder for the React Flow canvas. All inline zIndex values inside
 * the workflow editor come from here — no ad-hoc numbers.
 */
export const CANVAS_Z = {
  /** Decorative connector strokes behind node bodies. */
  behindNode: -1,
  /** Default edges. */
  edge: 1,
  /** Edges rendered inside containers (above the container background). */
  edgeInContainer: 1001,
  /** Edge labels. */
  edgeLabel: 1002,
  /** Hoverable edge controls (add-step button) — above labels. */
  edgeControls: 1003,
} as const;
