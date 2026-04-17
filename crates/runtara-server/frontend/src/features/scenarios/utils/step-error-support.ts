import type { ExtendedAgent } from '@/features/scenarios/queries';

/**
 * Determines if a step can have error handlers based on its type and configuration.
 *
 * @param stepType - The type of step (e.g., 'Agent', 'Create', 'Conditional')
 * @param agentId - The agent ID for Agent steps
 * @param capabilityId - The capability ID for Agent steps
 * @param agents - List of available agents with their capabilities
 * @returns true if the step can fail and should show an error handle
 */
export function canStepHaveErrorHandler(
  stepType: string,
  agentId?: string,
  capabilityId?: string,
  agents?: ExtendedAgent[]
): boolean {
  // Steps that cannot have error handlers:
  // - Finish/Start: workflow boundaries
  // - Conditional/Switch: evaluation steps that cannot fail
  // - Error: already an error handler, cannot have its own error handler
  const stepsWithoutErrorHandlers = [
    'Finish',
    'Start',
    'Conditional',
    'Switch',
    'Error',
  ];
  if (stepsWithoutErrorHandlers.includes(stepType)) {
    return false;
  }

  // For Agent steps, check if the capability has knownErrors defined
  if (stepType === 'Agent' && agentId && capabilityId && agents) {
    const agent = agents.find(
      (a) => a.id.toLowerCase() === agentId.toLowerCase()
    );
    if (!agent) {
      // If agent not found, assume it can have errors (safer default)
      return true;
    }

    const capability = agent.supportedCapabilities[capabilityId];
    if (!capability) {
      // If capability not found, assume it can have errors (safer default)
      return true;
    }

    // Check if the capability has knownErrors defined
    // If knownErrors array exists and has entries, show error handle
    // If knownErrors is undefined or empty, don't show error handle
    return !!(capability.knownErrors && capability.knownErrors.length > 0);
  }

  // For non-Agent steps that can fail (Create, Split, Combine, etc.)
  // Always show error handle since we don't have capability-level granularity
  return true;
}
