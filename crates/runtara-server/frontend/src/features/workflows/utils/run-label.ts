export function executionDisplayName(run: {
  runLabel?: string | null;
  workflowName?: string | null;
  workflowId?: string | null;
}) {
  return (
    run.runLabel || run.workflowName || run.workflowId || 'Ad-hoc invocation'
  );
}
