/**
 * Label for a folder row's workflow count.
 *
 * Counts are fetched per folder, so a row can render before its count arrives.
 * An unknown count renders as empty rather than "0 workflows" — a zero the user
 * would read as a real count, which is the exact failure this column used to
 * have when counts were tallied from a truncated page of workflows.
 */
export function folderCountLabel(count: number | undefined): string {
  if (count === undefined) return '';
  return `${count} workflow${count === 1 ? '' : 's'}`;
}
