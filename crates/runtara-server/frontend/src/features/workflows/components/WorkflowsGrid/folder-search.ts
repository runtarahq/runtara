/**
 * Search matching for the folder half of the workflow listing.
 *
 * Workflows are filtered server-side by the listing endpoint's `search`
 * parameter; folders come from a separate endpoint that returns the whole child
 * list at once, so their rows are matched here. Keep the rule the same as the
 * server's — case-insensitive substring on the name — or a search would filter
 * the two halves of one table by different rules.
 */

export interface SearchableFolder {
  name: string;
}

/** Folders whose name contains `searchTerm`; all of them when the term is blank. */
export function matchFolders<T extends SearchableFolder>(
  folders: readonly T[],
  searchTerm: string
): readonly T[] {
  const needle = searchTerm.trim().toLowerCase();
  if (!needle) return folders;

  return folders.filter((folder) =>
    (folder.name || '').toLowerCase().includes(needle)
  );
}
