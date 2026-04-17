import { SchemaField } from '../../EditorSidebar/SchemaFieldsEditor';
import { StepInfo, StepParameter } from '../shared';
import { SimpleVariable } from '../NodeFormContext';

export interface VariableSuggestion {
  label: string;
  value: string;
  description?: string;
  group: 'Scenario Inputs' | 'Variables' | 'Step Outputs' | 'Loop Context';
  type?: string;
  /** Step ID for step outputs (used to extract field path) */
  stepId?: string;
  /** Step name for display purposes */
  stepName?: string;
  /** Field path without the steps['id'].outputs prefix */
  fieldPath?: string;
}

/**
 * Flattens step parameters recursively for autocomplete suggestions
 */
function flattenStepParameters(
  parameters: StepParameter[],
  prefix: string = ''
): { path: string; type?: string }[] {
  const result: { path: string; type?: string }[] = [];

  for (const param of parameters) {
    const fullPath = prefix ? `${prefix}.${param.name}` : param.name;

    result.push({
      path: param.path,
      type: param.type,
    });

    if (param.children && param.children.length > 0) {
      result.push(...flattenStepParameters(param.children, fullPath));
    }
  }

  return result;
}

/**
 * Composes variable suggestions from scenario inputs, variables, and previous steps
 */
export function composeVariableSuggestions(
  previousSteps: StepInfo[],
  inputSchemaFields?: SchemaField[],
  variables?: SimpleVariable[],
  isInsideWhileLoop?: boolean
): VariableSuggestion[] {
  const suggestions: VariableSuggestion[] = [];

  // Add scenario input schema fields
  if (inputSchemaFields && inputSchemaFields.length > 0) {
    for (const field of inputSchemaFields) {
      if (field.name) {
        suggestions.push({
          label: field.name,
          value: `scenario.inputs.data.${field.name}`,
          description: field.description || 'Scenario input field',
          group: 'Scenario Inputs',
          type: field.type,
        });
      }
    }
  }

  // Always add generic scenario.inputs.data as fallback
  suggestions.push({
    label: 'data',
    value: 'scenario.inputs.data',
    description: 'All scenario input data',
    group: 'Scenario Inputs',
    type: 'object',
  });

  // Add built-in runtime variables (always available in all steps/subgraphs)
  suggestions.push({
    label: '_scenario_id',
    value: 'variables._scenario_id',
    description:
      'Scenario ID and instance ID (format: {scenario_id}::{instance_id})',
    group: 'Variables',
    type: 'string',
  });
  suggestions.push({
    label: '_instance_id',
    value: 'variables._instance_id',
    description: 'Execution instance UUID',
    group: 'Variables',
    type: 'string',
  });
  suggestions.push({
    label: '_tenant_id',
    value: 'variables._tenant_id',
    description: 'Tenant identifier',
    group: 'Variables',
    type: 'string',
  });

  // Add scenario variables
  if (variables && variables.length > 0) {
    for (const variable of variables) {
      if (variable.name) {
        suggestions.push({
          label: variable.name,
          value: `scenario.inputs.variables.${variable.name}`,
          description: variable.description || 'Scenario variable',
          group: 'Variables',
          type: variable.type?.toLowerCase(),
        });
      }
    }
  }

  // Add loop context references when inside a While loop
  if (isInsideWhileLoop) {
    suggestions.push({
      label: 'loop.index',
      value: 'loop.index',
      description: 'Current iteration counter (0-based)',
      group: 'Loop Context',
      type: 'number',
    });
    suggestions.push({
      label: 'loop.outputs',
      value: 'loop.outputs',
      description:
        'Finish step outputs from previous iteration (null on first)',
      group: 'Loop Context',
      type: 'object',
    });
  }

  // Add suggestions from previous steps' outputs
  for (const step of previousSteps ?? []) {
    const flattenedParams = flattenStepParameters(step.outputs);

    for (const param of flattenedParams) {
      // Extract field path from full path (e.g., "steps['id'].outputs.field" -> "field")
      const outputsPrefix = `steps['${step.id}'].outputs`;
      let fieldPath = param.path;
      if (param.path.startsWith(outputsPrefix)) {
        fieldPath = param.path.slice(outputsPrefix.length);
        // Remove leading dot if present
        if (fieldPath.startsWith('.')) {
          fieldPath = fieldPath.slice(1);
        }
      }

      suggestions.push({
        label: fieldPath || 'outputs',
        value: param.path,
        description: step.name,
        group: 'Step Outputs',
        type: param.type,
        stepId: step.id,
        stepName: step.name,
        fieldPath: fieldPath || 'outputs',
      });
    }
  }

  return suggestions;
}

/**
 * Filters suggestions based on search query
 */
export function filterSuggestions(
  suggestions: VariableSuggestion[],
  query: string
): VariableSuggestion[] {
  if (!query) {
    return suggestions;
  }

  const lowerQuery = query.toLowerCase();

  return suggestions.filter((suggestion) => {
    const lowerLabel = suggestion.label.toLowerCase();
    const lowerDescription = suggestion.description?.toLowerCase() || '';

    return (
      lowerLabel.includes(lowerQuery) || lowerDescription.includes(lowerQuery)
    );
  });
}

/**
 * Groups suggestions by their group property
 */
export function groupSuggestions(
  suggestions: VariableSuggestion[]
): Record<string, VariableSuggestion[]> {
  const grouped: Record<string, VariableSuggestion[]> = {
    'Loop Context': [],
    'Scenario Inputs': [],
    Variables: [],
    'Step Outputs': [],
  };

  for (const suggestion of suggestions) {
    grouped[suggestion.group].push(suggestion);
  }

  return grouped;
}
