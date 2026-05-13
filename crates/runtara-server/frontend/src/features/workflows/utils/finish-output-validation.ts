import type { Node } from '@xyflow/react';
import { v4 as uuidv4 } from 'uuid';

import type { ValidationMessage } from '../types/validation';

type MappingInput = {
  type?: unknown;
  value?: unknown;
  valueType?: unknown;
};

export type FinishOutputValidationIssue = {
  index: number;
  field: 'type' | 'value';
  path: Array<string | number>;
  message: string;
};

function isEmptyCompositeValue(value: unknown) {
  if (Array.isArray(value)) {
    return value.length === 0;
  }

  return (
    value !== undefined &&
    value !== null &&
    typeof value === 'object' &&
    Object.keys(value as Record<string, unknown>).length === 0
  );
}

function hasMappingValue(item: { value?: unknown; valueType?: unknown }) {
  if (item.value === null) {
    return item.valueType === 'immediate';
  }

  if (
    item.valueType === 'composite' &&
    item.value !== undefined &&
    item.value !== null
  ) {
    return !isEmptyCompositeValue(item.value);
  }

  return (
    item.value !== undefined &&
    item.value !== null &&
    !(typeof item.value === 'string' && item.value.trim() === '')
  );
}

function normalizeMapping(inputMapping: unknown): MappingInput[] {
  if (Array.isArray(inputMapping)) {
    return inputMapping as MappingInput[];
  }

  if (
    inputMapping &&
    typeof inputMapping === 'object' &&
    !Array.isArray(inputMapping)
  ) {
    return Object.entries(inputMapping as Record<string, unknown>).map(
      ([type, rawValue]) => {
        if (rawValue && typeof rawValue === 'object') {
          const value = rawValue as {
            value?: unknown;
            valueType?: unknown;
          };
          return {
            type,
            value: value.value,
            valueType: value.valueType,
          };
        }

        return {
          type,
          value: rawValue,
          valueType: 'immediate',
        };
      }
    );
  }

  return [];
}

export function getFinishOutputValidationIssues(stepData: {
  stepType?: unknown;
  inputMapping?: unknown;
}): FinishOutputValidationIssue[] {
  if (stepData.stepType !== 'Finish') {
    return [];
  }

  const outputs = normalizeMapping(stepData.inputMapping);
  return outputs.flatMap((item, index) => {
    const issues: FinishOutputValidationIssue[] = [];
    const hasName =
      typeof item.type === 'string' && item.type.trim().length > 0;
    const hasSource = hasMappingValue(item);

    if (!hasName) {
      issues.push({
        index,
        field: 'type',
        path: ['inputMapping', index, 'type'],
        message: 'Output name is required',
      });
    }

    if (!hasSource) {
      issues.push({
        index,
        field: 'value',
        path: ['inputMapping', index, 'value'],
        message: 'Source is required',
      });
    }

    return issues;
  });
}

export function getFinishOutputValidationMessages(
  nodes: Node[]
): ValidationMessage[] {
  return nodes.flatMap((node) => {
    const issues = getFinishOutputValidationIssues(node.data);
    if (issues.length === 0) {
      return [];
    }

    const stepName =
      typeof node.data?.name === 'string' ? node.data.name : undefined;

    return issues.map((issue) => ({
      id: uuidv4(),
      severity: 'error' as const,
      code: 'E_FINISH_OUTPUT_REQUIRED',
      message: `Finish output row ${issue.index + 1}: ${issue.message}`,
      stepId: node.id,
      stepName,
      fieldName: issue.field,
      source: 'client' as const,
      timestamp: Date.now(),
    }));
  });
}
