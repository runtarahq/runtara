/** One-line summaries shared by the canvas nodes and the timeline cards. */
export interface StepSummaryData {
  stepType?: string;
  agentId?: string;
  capabilityId?: string;
  description?: string;
}

/** What a step does, in one line: its description, or a summary by type. */
export function stepDescription(
  data: StepSummaryData,
  agentName?: string
): string | undefined {
  if (data.description) return data.description;
  switch (data.stepType) {
    case 'Agent':
      return `${agentName || data.agentId || 'agent'} / ${data.capabilityId || 'capability'}`;
    case 'Conditional':
      return 'Routes execution by condition.';
    case 'Switch':
      return 'Routes execution by value.';
    case 'Split':
      return 'Runs a subgraph for each item.';
    case 'While':
      return 'Repeats its subgraph while the condition is true.';
    case 'EmbedWorkflow':
      return 'Calls another workflow.';
    case 'Finish':
      return 'Completes this path.';
    default:
      return undefined;
  }
}

export function stepBadgeVariant(stepType: string) {
  switch (stepType) {
    case 'Conditional':
    case 'Switch':
      return 'warning' as const;
    case 'Split':
    case 'While':
    case 'RepeatUntil':
      return 'default' as const;
    case 'EmbedWorkflow':
      return 'secondary' as const;
    case 'Finish':
      return 'success' as const;
    default:
      return 'muted' as const;
  }
}
