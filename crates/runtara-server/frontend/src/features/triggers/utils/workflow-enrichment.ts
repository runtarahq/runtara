/**
 * Creates a Map of workflow IDs to workflow names for efficient lookup.
 * Used to enrich trigger data with human-readable workflow names.
 */
export function createWorkflowNameMap(
  workflows:
    | { id?: string; name?: string }[]
    | { data?: { id?: string; name?: string }[] }
    | { data?: { content?: { id?: string; name?: string }[] } }
): Map<string, string> {
  const map = new Map<string, string>();
  // Handle direct array, { data: [...] }, and paginated { data: { content: [...] } } formats
  const workflowsAny = workflows as any;
  const workflowList =
    workflowsAny?.data?.content || workflowsAny?.data || workflows || [];
  for (const workflow of workflowList) {
    if (workflow.id && workflow.name) {
      map.set(workflow.id, workflow.name);
    }
  }
  return map;
}
