import { humanizeKey } from './step-details';

/** One-line summaries shared by the canvas nodes and the timeline cards. */
export interface StepSummaryData {
  stepType?: string;
  agentId?: string;
  capabilityId?: string;
  description?: string;
}

/**
 * What a step does, in one line: its description, or a summary by type.
 * Agent steps read "Object Model · Update instance": the agent's and the
 * capability's display names when known, humanized ids otherwise.
 */
export function stepDescription(
  data: StepSummaryData,
  agentName?: string,
  capabilityName?: string
): string | undefined {
  if (data.description) return data.description;
  switch (data.stepType) {
    case 'Agent': {
      const agent =
        agentName || (data.agentId ? humanizeKey(data.agentId) : 'Agent');
      const capability =
        capabilityName ||
        (data.capabilityId ? humanizeKey(data.capabilityId) : '');
      return capability ? `${agent} · ${capability}` : agent;
    }
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

/** `WaitForSignal` → "Wait for signal", `AiAgent` → "AI agent". */
export function stepTypeLabel(stepType: string): string {
  if (stepType === 'AiAgent') return 'AI agent';
  return humanizeKey(stepType);
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
