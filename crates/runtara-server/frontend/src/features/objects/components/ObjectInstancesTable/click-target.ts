/**
 * Did a click land on the editable grid itself?
 *
 * A pending inline edit is flushed when the user clicks away from the grid, so
 * this decides whether an edit gets written. It deliberately measures against
 * the `<table>` rather than the console shell: the shell also contains the
 * breadcrumb, toolbar and footer, and treating the whole shell as "the table"
 * meant almost every click counted as inside — a pending edit was only ever
 * flushed by clicking the sidebar or a portal-rendered dialog, so in practice
 * edits were silently dropped.
 */
export function clickLandedInGrid(
  target: Node | null | undefined,
  shell: HTMLElement | null | undefined
): boolean {
  const grid = shell?.querySelector('table');
  return !!(target && grid?.contains(target));
}
