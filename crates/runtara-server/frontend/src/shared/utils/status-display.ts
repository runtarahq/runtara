/**
 * Status display utility for workflow instances
 * Maps status and terminationType to display properties
 */

interface StatusDisplayInfo {
  text: string;
  variant: 'default' | 'secondary' | 'destructive' | 'outline';
  showSpinner: boolean;
}

interface TerminationTypeDisplayInfo {
  text: string;
  variant: 'default' | 'secondary' | 'destructive' | 'outline';
}

/**
 * Get display information for a workflow instance status
 * Returns only the status information (separate from termination type)
 */
export function getStatusDisplay(
  status: string | undefined | null
): StatusDisplayInfo {
  const normalizedStatus = status?.toLowerCase() || 'unknown';

  // Completed successfully
  if (normalizedStatus === 'completed' || normalizedStatus === 'success') {
    return {
      text: 'Completed',
      variant: 'default',
      showSpinner: false,
    };
  }

  // Failed states
  if (normalizedStatus === 'failed' || normalizedStatus === 'error') {
    return {
      text: 'Failed',
      variant: 'destructive',
      showSpinner: false,
    };
  }

  // Cancelled states
  if (normalizedStatus === 'cancelled' || normalizedStatus === 'aborted') {
    return {
      text: 'Cancelled',
      variant: 'outline',
      showSpinner: false,
    };
  }

  // Timeout states
  if (normalizedStatus === 'timeout') {
    return {
      text: 'Timeout',
      variant: 'destructive',
      showSpinner: false,
    };
  }

  // Active states - show spinner
  if (normalizedStatus === 'running') {
    return {
      text: 'Running',
      variant: 'secondary',
      showSpinner: true,
    };
  }

  if (normalizedStatus === 'compiling') {
    return {
      text: 'Compiling',
      variant: 'secondary',
      showSpinner: true,
    };
  }

  if (normalizedStatus === 'queued' || normalizedStatus === 'pending') {
    return {
      text: 'Queued',
      variant: 'outline',
      showSpinner: true,
    };
  }

  if (normalizedStatus === 'suspended') {
    return {
      text: 'Suspended',
      variant: 'outline',
      showSpinner: false,
    };
  }

  if (normalizedStatus === 'not_started') {
    return {
      text: 'Not started',
      variant: 'outline',
      showSpinner: false,
    };
  }

  // Unknown/default state
  return {
    text: 'Unknown',
    variant: 'outline',
    showSpinner: false,
  };
}

/**
 * Get display information for termination type
 * Returns null if no termination type or if it's normal_completion
 */
export function getTerminationTypeDisplay(
  terminationType: string | undefined | null
): TerminationTypeDisplayInfo | null {
  if (!terminationType) {
    return null;
  }

  // Don't show termination type for normal completions
  if (terminationType === 'normal_completion') {
    return null;
  }

  switch (terminationType) {
    case 'user_initiated':
      return {
        text: 'User Cancelled',
        variant: 'outline',
      };
    case 'queue_timeout':
      return {
        text: 'Queue Timeout (24h)',
        variant: 'destructive',
      };
    case 'execution_timeout':
      return {
        text: 'Execution Timeout',
        variant: 'destructive',
      };
    case 'system_error':
      return {
        text: 'System Error',
        variant: 'destructive',
      };
    default:
      return {
        text: terminationType,
        variant: 'outline',
      };
  }
}

/**
 * Check if a status represents an active (non-terminal) state
 * Active states can be cancelled
 */
export function isActiveStatus(status: string | undefined | null): boolean {
  const normalizedStatus = status?.toLowerCase() || '';
  return [
    'queued',
    'compiling',
    'running',
    'pending',
    'suspended',
    'unknown',
  ].includes(normalizedStatus);
}

/**
 * Check if a status represents a terminal (final) state
 * Terminal states cannot be cancelled
 */
export function isTerminalStatus(status: string | undefined | null): boolean {
  return !isActiveStatus(status);
}
