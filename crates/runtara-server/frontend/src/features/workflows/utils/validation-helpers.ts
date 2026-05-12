import { v4 as uuidv4 } from 'uuid';
import { Node } from '@xyflow/react';
import { ValidationMessage, ValidationSeverity } from '../types/validation';
import { ValidationError } from '@/shared/hooks/api';
import { detectErrorCode, detectWarningCode } from './validation-codes';

/**
 * Extract step name from an error message.
 * Client validation errors often include step names in the message.
 */
function extractStepNamesFromMessage(message: string): string[] {
  // Look for quoted names or names after colons
  const quotedMatch = message.match(/"([^"]+)"/g);
  if (quotedMatch) {
    return quotedMatch.map((m) => m.replace(/"/g, ''));
  }

  const singleQuotedStepMatch = message.match(/Step '([^']+)'/gi);
  if (singleQuotedStepMatch) {
    return singleQuotedStepMatch
      .map((m) => m.match(/Step '([^']+)'/i)?.[1])
      .filter((value): value is string => Boolean(value));
  }

  // Look for names after "Step: " or similar patterns
  const stepMatch = message.match(/step[:\s]+([^,.\n]+)/i);
  if (stepMatch) {
    return [stepMatch[1].trim()];
  }

  // Look for names after colons at the end
  const colonMatch = message.match(/:\s*(.+)$/);
  if (colonMatch) {
    return colonMatch[1].split(',').map((s) => s.trim());
  }

  return [];
}

/**
 * Find step ID from step name by searching nodes.
 */
function findStepIdByName(stepName: string, nodes: Node[]): string | undefined {
  const node = nodes.find(
    (n) =>
      (n.data?.name as string)?.toLowerCase() === stepName.toLowerCase() ||
      n.id === stepName
  );
  return node?.id;
}

/**
 * Get step name from node by ID.
 */
function getStepNameById(stepId: string, nodes: Node[]): string | undefined {
  const node = nodes.find((n) => n.id === stepId);
  return node?.data?.name as string | undefined;
}

/**
 * Convert frontend-side validation errors to ValidationMessage format.
 * This includes canonical Rust/WASM validation that runs in the browser.
 */
export function convertClientErrors(
  errors: string[],
  nodes: Node[]
): ValidationMessage[] {
  return errors.map((errorMessage) => {
    // Try to extract step info from error message
    const stepNames = extractStepNamesFromMessage(errorMessage);

    // Find step ID from names
    let stepId: string | undefined;
    let stepName: string | undefined;

    if (stepNames.length > 0) {
      for (const name of stepNames) {
        stepId = findStepIdByName(name, nodes);
        if (stepId) {
          stepName = name;
          break;
        }
      }
      // If no ID found but we have names, use the first name
      if (!stepName && stepNames.length > 0) {
        stepName = stepNames[0];
      }
    }

    return {
      id: uuidv4(),
      severity: 'error' as ValidationSeverity,
      code: detectErrorCode(errorMessage),
      message: errorMessage,
      stepId: stepId || null,
      stepName,
      source: 'client' as const,
      timestamp: Date.now(),
    };
  });
}

/**
 * Convert frontend-side validation warnings to ValidationMessage format.
 * This includes canonical Rust/WASM validation that runs in the browser.
 */
export function convertClientWarnings(
  warnings: string[],
  nodes: Node[]
): ValidationMessage[] {
  return warnings.map((warningMessage) => {
    // Try to extract step info from warning message
    const stepNames = extractStepNamesFromMessage(warningMessage);

    // Find step ID from names
    let stepId: string | undefined;
    let stepName: string | undefined;

    if (stepNames.length > 0) {
      for (const name of stepNames) {
        stepId = findStepIdByName(name, nodes);
        if (stepId) {
          stepName = name;
          break;
        }
      }
      if (!stepName && stepNames.length > 0) {
        stepName = stepNames[0];
      }
    }

    return {
      id: uuidv4(),
      severity: 'warning' as ValidationSeverity,
      code: detectWarningCode(warningMessage),
      message: warningMessage,
      stepId: stepId || null,
      stepName,
      source: 'client' as const,
      timestamp: Date.now(),
    };
  });
}

/**
 * Convert server-side ValidationError to ValidationMessage format.
 */
export function convertServerErrors(
  errors: ValidationError[],
  nodes: Node[]
): ValidationMessage[] {
  return errors.map((error) => {
    const stepName = error.stepId
      ? getStepNameById(error.stepId, nodes)
      : undefined;

    return {
      id: uuidv4(),
      severity: 'error' as ValidationSeverity,
      code: error.code,
      message: error.message,
      stepId: error.stepId || null,
      stepName,
      fieldName: error.fieldName,
      relatedStepIds: error.relatedStepIds || undefined,
      source: 'server' as const,
      timestamp: Date.now(),
    };
  });
}
