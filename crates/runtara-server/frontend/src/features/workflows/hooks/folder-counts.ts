import type { FolderWorkflowCount } from '@/generated/RuntaraRuntimeApi';

/**
 * Recursive workflow count for one folder: its own workflows plus everything in
 * its subfolders.
 *
 * The server reports counts per exact path, so a folder's true total is the sum
 * of every path that sits underneath it. Folder paths always carry a trailing
 * slash, which is what makes the prefix test safe — "/Demo/Test/" starts with
 * "/Demo/", while "/DemoArchive/" does not.
 *
 * This also covers folders the server never reports. `parseFolderPaths`
 * synthesizes intermediate folders so a nested tree stays navigable, and such a
 * folder holds no workflows of its own; summing by prefix still gives it the
 * right total.
 */
export function recursiveWorkflowCount(
  counts: readonly FolderWorkflowCount[],
  folderPath: string
): number {
  return counts.reduce(
    (total, entry) =>
      entry.path.startsWith(folderPath) ? total + entry.workflowCount : total,
    0
  );
}

/**
 * Recursive counts for each of `folderPaths`, keyed by path.
 *
 * Returns an empty record when counts have not loaded, so callers can tell
 * "not known yet" from a real zero.
 */
export function buildFolderCounts(
  counts: readonly FolderWorkflowCount[] | undefined,
  folderPaths: readonly string[]
): Record<string, number> {
  if (!counts) return {};

  const result: Record<string, number> = {};
  folderPaths.forEach((path) => {
    result[path] = recursiveWorkflowCount(counts, path);
  });
  return result;
}
