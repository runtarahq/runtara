import {
  ErrorCategory,
  ErrorSeverity,
} from '@/generated/RuntaraRuntimeApi';

/**
 * Structured error format returned by the backend.
 * Error strings in API responses may contain JSON-serialized StructuredError objects.
 */
export interface StructuredError {
  /** Machine-readable error code (e.g., "OPENAI_RATE_LIMITED") */
  code: string;
  /** Human-readable error message */
  message: string;
  /** Error category determines retry behavior */
  category: ErrorCategory;
  /** Error severity level */
  severity: ErrorSeverity;
  /** Additional context (status_code, errors, user_errors, etc.) */
  attributes: Record<string, unknown>;
}

/**
 * Error type classification for UI display
 */
export type ErrorType = 'transient' | 'technical' | 'business';
