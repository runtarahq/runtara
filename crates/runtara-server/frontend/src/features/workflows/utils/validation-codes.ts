/**
 * Client-side validation error and warning codes.
 * These codes are used to categorize validation messages for display in the validation panel.
 */

/**
 * Error codes (E-series) - Block save operation
 */
const CLIENT_ERROR_CODES = {
  /** Workflow has no steps */
  NO_STEPS: 'E001',
  /** Workflow has no entry point (no step with incoming edges) */
  NO_ENTRY_POINT: 'E002',
  /** Step is not connected to the workflow */
  ORPHANED_NODE: 'E003',
  /** Finish step has outgoing edges */
  FINISH_HAS_OUTGOING: 'E004',
  /** Deprecated Start step still in use */
  DEPRECATED_START_STEP: 'E005',
  /** Self-connection detected */
  SELF_CONNECTION: 'E006',
  /** Circular dependency detected */
  CIRCULAR_DEPENDENCY: 'E007',
  /** Unknown client validation error */
  UNKNOWN: 'E000',
} as const;

/**
 * Warning codes (W-series) - Don't block save, but inform user
 */
const CLIENT_WARNING_CODES = {
  /** Step has no description */
  NO_DESCRIPTION: 'W001',
  /** Unused variable defined */
  UNUSED_VARIABLE: 'W002',
  /** Step has very long name */
  LONG_STEP_NAME: 'W003',
  /** Disconnected subgraph detected */
  DISCONNECTED_SUBGRAPH: 'W004',
  /** Unknown client warning */
  UNKNOWN: 'W000',
} as const;

function detectPrefixedValidationCode(
  message: string,
  prefix: 'E' | 'W'
): string | null {
  const match = message.match(new RegExp(`\\[(${prefix}\\d{3})\\]`));
  return match?.[1] ?? null;
}

/**
 * Map error message patterns to error codes.
 * Used when converting client validation string errors to structured messages.
 */
export function detectErrorCode(message: string): string {
  const explicitCode = detectPrefixedValidationCode(message, 'E');
  if (explicitCode) {
    return explicitCode;
  }

  const lowerMessage = message.toLowerCase();

  if (lowerMessage.includes('at least one step')) {
    return CLIENT_ERROR_CODES.NO_STEPS;
  }
  if (lowerMessage.includes('entry point')) {
    return CLIENT_ERROR_CODES.NO_ENTRY_POINT;
  }
  if (
    lowerMessage.includes('not connected') ||
    lowerMessage.includes('orphan')
  ) {
    return CLIENT_ERROR_CODES.ORPHANED_NODE;
  }
  if (lowerMessage.includes('finish') && lowerMessage.includes('outgoing')) {
    return CLIENT_ERROR_CODES.FINISH_HAS_OUTGOING;
  }
  if (lowerMessage.includes('deprecated') && lowerMessage.includes('start')) {
    return CLIENT_ERROR_CODES.DEPRECATED_START_STEP;
  }
  if (lowerMessage.includes('self-connection')) {
    return CLIENT_ERROR_CODES.SELF_CONNECTION;
  }
  if (lowerMessage.includes('circular') || lowerMessage.includes('loop')) {
    return CLIENT_ERROR_CODES.CIRCULAR_DEPENDENCY;
  }

  return CLIENT_ERROR_CODES.UNKNOWN;
}

/**
 * Map warning message patterns to warning codes.
 */
export function detectWarningCode(message: string): string {
  const explicitCode = detectPrefixedValidationCode(message, 'W');
  if (explicitCode) {
    return explicitCode;
  }

  const lowerMessage = message.toLowerCase();

  if (lowerMessage.includes('no description')) {
    return CLIENT_WARNING_CODES.NO_DESCRIPTION;
  }
  if (lowerMessage.includes('unused variable')) {
    return CLIENT_WARNING_CODES.UNUSED_VARIABLE;
  }
  if (lowerMessage.includes('long name')) {
    return CLIENT_WARNING_CODES.LONG_STEP_NAME;
  }
  if (lowerMessage.includes('disconnected')) {
    return CLIENT_WARNING_CODES.DISCONNECTED_SUBGRAPH;
  }

  return CLIENT_WARNING_CODES.UNKNOWN;
}
