// Workflow Editor Configuration

/**
 * Grid size for snap-to-grid functionality.
 * This value is used for:
 * - Node dragging snap interval
 * - Node resizing snap interval
 * - Background grid visual spacing
 *
 * Keeping this consistent ensures that resizing and positioning
 * align properly with the visual grid.
 */
export const SNAP_GRID_SIZE = 12;

/**
 * Snaps a single numeric value to the grid.
 * @param value - The value to snap
 * @returns The value rounded to the nearest grid increment
 */
export const snapToGrid = (value: number): number => {
  return Math.round(value / SNAP_GRID_SIZE) * SNAP_GRID_SIZE;
};

/**
 * Snaps a position object to the grid.
 * @param position - The position to snap with x and y coordinates
 * @returns A new position object with both coordinates snapped to grid
 */
export const snapPositionToGrid = (position: {
  x: number;
  y: number;
}): { x: number; y: number } => {
  return {
    x: snapToGrid(position.x),
    y: snapToGrid(position.y),
  };
};

/**
 * Snaps a container height to an odd multiple of SNAP_GRID_SIZE.
 * This ensures the container's half-height has the same mod-12 remainder (6)
 * as BasicNode's half-height (18), so their centers align perfectly on the
 * 12px grid without requiring non-grid Y positions.
 *
 * Math: for centers to align on grid, half-heights must share the same mod-12.
 * BasicNode half = 18 (18 mod 12 = 6). Container half = h/2.
 * h/2 mod 12 = 6 when h is an odd multiple of 12 (i.e., 12, 36, 60, 84, 108, 132, ...).
 */
export const snapContainerHeightToGrid = (height: number): number => {
  // Odd multiples of 12: 12, 36, 60, 84, 108, 132, 156, ...
  // These are values of form 24k + 12.
  const k = Math.round((height - SNAP_GRID_SIZE) / (SNAP_GRID_SIZE * 2));
  return Math.max(SNAP_GRID_SIZE, k * SNAP_GRID_SIZE * 2 + SNAP_GRID_SIZE);
};
