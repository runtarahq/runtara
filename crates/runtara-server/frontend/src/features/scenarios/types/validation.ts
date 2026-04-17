/**
 * Validation types for the scenario editor validation panel.
 * These types unify client-side and server-side validation messages.
 */

export type ValidationSeverity = 'error' | 'warning';

export type ValidationFilter = 'all' | 'errors' | 'warnings';

export interface ValidationMessage {
  /** Unique identifier for the message */
  id: string;

  /** Severity level - errors block save, warnings don't */
  severity: ValidationSeverity;

  /** Error/warning code (e.g., "E023", "W001") */
  code: string;

  /** Human-readable message */
  message: string;

  /** Step ID where the issue occurred (if applicable) */
  stepId?: string | null;

  /** Step name for display (resolved from workflow nodes) */
  stepName?: string;

  /** Field name within the step (if applicable) */
  fieldName?: string | null;

  /** Additional step IDs involved (for multi-step issues) */
  relatedStepIds?: string[];

  /** Source of the validation message */
  source: 'client' | 'server';

  /** Timestamp when the message was generated */
  timestamp: number;
}
