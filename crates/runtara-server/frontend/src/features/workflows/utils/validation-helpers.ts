import { v4 as uuidv4 } from 'uuid';
import { Node } from '@xyflow/react';
import { ValidationMessage, ValidationSeverity } from '../types/validation';
import { ValidationError } from '@/shared/hooks/api';
import { detectErrorCode, detectWarningCode } from './validation-codes';
import { createGraphNodeResolver } from './step-node-resolver';

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
  nodes: Node[],
  graphPaths?: string[][]
): ValidationMessage[] {
  const findStepNode = createGraphNodeResolver(nodes);
  return errors.map((errorMessage, index) => {
    // Try to extract step info from error message
    const stepNames = extractStepNamesFromMessage(errorMessage);

    // Find step ID from names
    let stepId: string | undefined;
    let stepName: string | undefined;

    if (stepNames.length > 0) {
      for (const name of stepNames) {
        stepId = findStepNode(name, graphPaths?.[index]);
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
  nodes: Node[],
  graphPaths?: string[][]
): ValidationMessage[] {
  const findStepNode = createGraphNodeResolver(nodes);
  return warnings.map((warningMessage, index) => {
    // Try to extract step info from warning message
    const stepNames = extractStepNamesFromMessage(warningMessage);

    // Find step ID from names
    let stepId: string | undefined;
    let stepName: string | undefined;

    if (stepNames.length > 0) {
      for (const name of stepNames) {
        stepId = findStepNode(name, graphPaths?.[index]);
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
  const findStepNode = createGraphNodeResolver(nodes);
  return errors.map((error) => {
    // Server errors name the DSL step id; resolve it to the canvas node in
    // the reported body (a step id may repeat across bodies).
    const stepId = error.stepId
      ? (findStepNode(error.stepId, error.graphPath) ?? error.stepId)
      : null;
    const stepName = stepId ? getStepNameById(stepId, nodes) : undefined;

    return {
      id: uuidv4(),
      severity: 'error' as ValidationSeverity,
      code: error.code,
      message: error.message,
      stepId,
      stepName,
      fieldName: error.fieldName,
      relatedStepIds:
        error.relatedStepIds?.map(
          (id) => findStepNode(id, error.graphPath) ?? id
        ) || undefined,
      source: 'server' as const,
      timestamp: Date.now(),
    };
  });
}
