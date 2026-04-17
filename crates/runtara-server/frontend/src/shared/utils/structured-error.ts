import {
  ErrorCategory,
  ErrorSeverity,
} from '@/generated/RuntaraRuntimeApi';
import type {
  StructuredError,
  ErrorType,
} from '@/shared/types/structured-error';

/**
 * Parse a potentially JSON-serialized error string into a StructuredError object.
 * Returns null if the string is not a valid structured error (legacy plain string).
 *
 * @param errorString - Error string that may contain JSON-serialized structured error
 * @returns Parsed StructuredError or null for legacy errors
 *
 * @example
 * ```typescript
 * const error = parseStructuredError(execution.error);
 * if (error) {
 *   console.log(`[${error.code}] ${error.message}`);
 * } else {
 *   console.log(execution.error); // Legacy plain string
 * }
 * ```
 */
export function parseStructuredError(
  errorString: string | null | undefined
): StructuredError | null {
  if (!errorString) {
    return null;
  }

  try {
    const parsed = JSON.parse(errorString);
    if (isStructuredError(parsed)) {
      return {
        ...parsed,
        // Backend may send `context` instead of `attributes` (e.g., Error step)
        attributes:
          parsed.attributes ??
          ((parsed as unknown as Record<string, unknown>).context as Record<
            string,
            unknown
          >) ??
          {},
      };
    }
  } catch {
    // Not JSON - legacy plain string error
  }

  return null;
}

/**
 * Type guard to check if a value is a valid StructuredError.
 *
 * @param value - Value to check
 * @returns True if value is a StructuredError
 */
export function isStructuredError(value: unknown): value is StructuredError {
  return (
    typeof value === 'object' &&
    value !== null &&
    'code' in value &&
    typeof value.code === 'string' &&
    'message' in value &&
    typeof value.message === 'string' &&
    'category' in value &&
    (value.category === ErrorCategory.Transient ||
      value.category === ErrorCategory.Permanent) &&
    'severity' in value &&
    (value.severity === ErrorSeverity.Info ||
      value.severity === ErrorSeverity.Warning ||
      value.severity === ErrorSeverity.Error ||
      value.severity === ErrorSeverity.Critical)
  );
}

/**
 * Classify error into transient, technical, or business type for UI display.
 *
 * Classification rules:
 * - Transient: Temporary failures that can be retried
 * - Technical: Permanent technical errors (severity: error/critical)
 * - Business: Permanent business logic errors (severity: warning)
 *
 * @param error - Structured error to classify
 * @returns Error type classification
 *
 * @example
 * ```typescript
 * const type = getErrorType(error);
 * if (type === 'transient') {
 *   showRetryButton();
 * } else if (type === 'business') {
 *   showWarningIcon();
 * }
 * ```
 */
export function getErrorType(error: StructuredError): ErrorType {
  if (error.category === ErrorCategory.Transient) {
    return 'transient';
  }

  // Permanent errors: distinguish technical vs business by severity
  if (error.severity === ErrorSeverity.Warning) {
    return 'business';
  }

  return 'technical';
}

/**
 * Determine if an error string represents a transient error that should show a retry button.
 *
 * @param errorString - Error string to check
 * @returns True if error is transient and retryable
 *
 * @example
 * ```typescript
 * if (shouldShowRetryButton(execution.error)) {
 *   return <Button onClick={handleRetry}>Retry</Button>;
 * }
 * ```
 */
export function shouldShowRetryButton(
  errorString: string | null | undefined
): boolean {
  const structured = parseStructuredError(errorString);
  return structured?.category === ErrorCategory.Transient;
}

/**
 * Calculate suggested retry delay in milliseconds based on error type.
 * Returns null if error should not be retried.
 *
 * Delay strategy:
 * - Rate limit errors: 60 seconds
 * - Other transient errors: 5 seconds
 * - Permanent errors: null (no retry)
 *
 * @param errorString - Error string to analyze
 * @returns Suggested delay in milliseconds, or null if should not retry
 *
 * @example
 * ```typescript
 * const delay = getRetryDelay(error);
 * if (delay) {
 *   setTimeout(handleRetry, delay);
 * }
 * ```
 */
export function getRetryDelay(
  errorString: string | null | undefined
): number | null {
  const structured = parseStructuredError(errorString);

  if (!structured || structured.category !== ErrorCategory.Transient) {
    return null;
  }

  // Suggest longer delays for rate limits
  if (structured.code.includes('RATE_LIMITED')) {
    return 60000; // 1 minute
  }

  return 5000; // 5 seconds for other transient errors
}

/**
 * Get UI badge variant based on error category.
 *
 * Variant mapping:
 * - Transient: "secondary" (orange/warning color)
 * - Permanent + error/critical: "destructive" (red)
 * - Permanent + warning: "outline" (yellow/muted)
 *
 * @param error - Structured error
 * @returns Badge variant for UI display
 */
export function getErrorBadgeVariant(
  error: StructuredError
): 'default' | 'destructive' | 'outline' | 'secondary' {
  const errorType = getErrorType(error);

  switch (errorType) {
    case 'transient':
      return 'secondary'; // Orange/warning color
    case 'business':
      return 'outline'; // Yellow/muted color
    case 'technical':
      return 'destructive'; // Red color
    default:
      return 'default';
  }
}

/**
 * Get user-friendly label for error category.
 *
 * @param category - Error category
 * @returns Display label
 */
export function getErrorCategoryLabel(category: ErrorCategory): string {
  switch (category) {
    case ErrorCategory.Transient:
      return 'Transient';
    case ErrorCategory.Permanent:
      return 'Permanent';
    default:
      return 'Unknown';
  }
}

/**
 * Get user-friendly label for error severity.
 *
 * @param severity - Error severity
 * @returns Display label
 */
export function getErrorSeverityLabel(severity: ErrorSeverity): string {
  switch (severity) {
    case ErrorSeverity.Info:
      return 'Info';
    case ErrorSeverity.Warning:
      return 'Warning';
    case ErrorSeverity.Error:
      return 'Error';
    case ErrorSeverity.Critical:
      return 'Critical';
    default:
      return 'Unknown';
  }
}
