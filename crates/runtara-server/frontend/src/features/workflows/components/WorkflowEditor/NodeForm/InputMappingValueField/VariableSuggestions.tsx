import { SchemaField } from '../../EditorSidebar/SchemaFieldsEditor';
import { StepInfo, StepParameter } from '../shared';
import { SimpleVariable } from '../NodeFormContext';

export interface VariableSuggestion {
  label: string;
  value: string;
  description?: string;
  group:
    | 'Workflow Inputs'
    | 'Variables'
    | 'Step Outputs'
    | 'Iteration Context'
    | 'Loop Context'
    | 'Split Scope'
    | 'Wait Scope'
    | 'Current Item';
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
 * Recursively turns schema fields (including nested object `properties`)
 * into dotted suggestions under `<valuePrefix>.<path>`.
 */
function appendSchemaFieldSuggestions(
  fields: SchemaField[],
  pathPrefix: string,
  valuePrefix: string,
  group: VariableSuggestion['group'],
  defaultDescription: string,
  suggestions: VariableSuggestion[]
): void {
  for (const field of fields) {
    if (!field.name) {
      continue;
    }
    const path = pathPrefix ? `${pathPrefix}.${field.name}` : field.name;
    suggestions.push({
      label: path,
      value: `${valuePrefix}.${path}`,
      description: field.description || defaultDescription,
      group,
      type: field.type,
    });
    if (field.properties && field.properties.length > 0) {
      appendSchemaFieldSuggestions(
        field.properties,
        path,
        valuePrefix,
        group,
        defaultDescription,
        suggestions
      );
    }
  }
}

/**
 * Composes variable suggestions from workflow inputs, variables, and previous steps
 */
export function composeVariableSuggestions(
  previousSteps: StepInfo[],
  inputSchemaFields?: SchemaField[],
  variables?: SimpleVariable[],
  isInsideWhileLoop?: boolean,
  isInsideSplit?: boolean,
  isInsideWaitScope?: boolean,
  splitItemSchemaFields?: SchemaField[]
): VariableSuggestion[] {
  const suggestions: VariableSuggestion[] = [];

  if (isInsideSplit) {
    // Inside a Split body the DSL rebinds `data.*` to the current iteration
    // item — the workflow-level input schema does not apply here, so instead
    // of offering wrong-scope workflow.inputs.data.* entries we surface the
    // Split's declared iteration schema (when the author declared one).
    suggestions.push({
      label: 'data',
      value: 'data',
      description: 'Current iteration item',
      group: 'Split Scope',
      type:
        splitItemSchemaFields && splitItemSchemaFields.length > 0
          ? 'object'
          : undefined,
    });
    if (splitItemSchemaFields && splitItemSchemaFields.length > 0) {
      appendSchemaFieldSuggestions(
        splitItemSchemaFields,
        '',
        'data',
        'Split Scope',
        'Iteration item field',
        suggestions
      );
    }
  } else if (isInsideWaitScope) {
    // Inside a WaitForSignal onWait subgraph, `data.*` is scoped to the onWait
    // graph's own input schema (DataScope::RequireSchema), which the editor
    // doesn't model — offer a bare, untyped `data` rather than the wrong
    // workflow-level inputs.
    suggestions.push({
      label: 'data',
      value: 'data',
      description: 'onWait scope input data',
      group: 'Wait Scope',
    });
  } else {
    // Add workflow input schema fields, expanding nested object properties
    // (declared via the schema editor's Advanced dialog) into dotted paths so
    // workflow.inputs.data.customer.email is offered and typed.
    if (inputSchemaFields && inputSchemaFields.length > 0) {
      appendSchemaFieldSuggestions(
        inputSchemaFields,
        '',
        'workflow.inputs.data',
        'Workflow Inputs',
        'Workflow input field',
        suggestions
      );
    }

    // Always add generic workflow.inputs.data as fallback
    suggestions.push({
      label: 'data',
      value: 'workflow.inputs.data',
      description: 'All workflow input data',
      group: 'Workflow Inputs',
      type: 'object',
    });
  }

  // Add built-in runtime variables (always available in all steps/subgraphs)
  suggestions.push({
    label: '_workflow_id',
    value: 'variables._workflow_id',
    description:
      'Workflow ID and instance ID (format: {workflow_id}::{instance_id})',
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

  // Add workflow variables
  if (variables && variables.length > 0) {
    for (const variable of variables) {
      if (variable.name) {
        suggestions.push({
          label: variable.name,
          value: `workflow.inputs.variables.${variable.name}`,
          description: variable.description || 'Workflow variable',
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

  // Uniform context for both Split and While bodies. `indices` is ordered
  // outermost-first, and `item` remains the nearest active Split item (or
  // null when the nesting stack contains only While steps).
  if (isInsideWhileLoop || isInsideSplit) {
    suggestions.push({
      label: 'iteration.index',
      value: 'iteration.index',
      description: '0-based index of the innermost Split or While iteration',
      group: 'Iteration Context',
      type: 'integer',
    });
    suggestions.push({
      label: 'iteration.indices',
      value: 'iteration.indices',
      description: 'All active Split/While indices, outermost first',
      group: 'Iteration Context',
      type: 'array',
    });
    suggestions.push({
      label: 'iteration.item',
      value: 'iteration.item',
      description: 'Nearest active Split item, or null when none is active',
      group: 'Iteration Context',
      type:
        isInsideSplit && splitItemSchemaFields?.length ? 'object' : undefined,
    });
    if (isInsideSplit && splitItemSchemaFields?.length) {
      appendSchemaFieldSuggestions(
        splitItemSchemaFields,
        '',
        'iteration.item',
        'Iteration Context',
        'Nearest Split item field',
        suggestions
      );
    }
  }

  // Add Split iteration scope variables when inside a Split subgraph.
  // The runtime injects these into each iteration's variables
  // (see SPLIT_SCOPE_VARIABLES in crates/runtara-workflows/src/validation.rs:
  // _index, _item, _loop, _loop_indices — referenceable as `variables.<name>`).
  if (isInsideSplit) {
    suggestions.push({
      label: 'variables._item',
      value: 'variables._item',
      description: 'Current array item for this Split iteration',
      group: 'Split Scope',
    });
    suggestions.push({
      label: 'variables._index',
      value: 'variables._index',
      description: '0-based index of the current Split iteration',
      group: 'Split Scope',
      type: 'number',
    });
    suggestions.push({
      label: 'variables._loop',
      value: 'variables._loop',
      description:
        'Enclosing While loop context ({index, outputs}); null unless the Split is nested in a While loop',
      group: 'Split Scope',
      type: 'object',
    });
    suggestions.push({
      label: 'variables._loop_indices',
      value: 'variables._loop_indices',
      description:
        'Iteration indices of all enclosing loop scopes, outermost first',
      group: 'Split Scope',
      type: 'array',
    });
  }

  // Add WaitForSignal onWait scope variables when inside an onWait subgraph.
  // The runtime injects these into the scope before the workflow suspends
  // (see wait_on_wait_variables in runtara-workflow-stdlib/src/direct_json.rs
  // and WAIT_ON_WAIT_SCOPE_VARIABLES in crates/runtara-workflows/src/validation.rs:
  // _signal_id — `_instance_id` is also injected but is a global built-in).
  if (isInsideWaitScope) {
    suggestions.push({
      label: 'variables._signal_id',
      value: 'variables._signal_id',
      description:
        'Signal id external systems must use to resume this WaitForSignal step',
      group: 'Wait Scope',
      type: 'string',
    });
  }

  // Add suggestions from previous steps' outputs
  for (const step of previousSteps ?? []) {
    const flattenedParams = flattenStepParameters(step.outputs);

    for (const param of flattenedParams) {
      // Extract field path from full path (e.g., "steps['id'].outputs.field" -> "field")
      const outputsPrefix = `steps['${step.id}'].outputs`;
      const stepPrefix = `steps['${step.id}'].`;
      let fieldPath = param.path;
      if (param.path.startsWith(outputsPrefix)) {
        fieldPath = param.path.slice(outputsPrefix.length);
        // Remove leading dot if present
        if (fieldPath.startsWith('.')) {
          fieldPath = fieldPath.slice(1);
        }
      } else if (param.path.startsWith(stepPrefix)) {
        // Sibling fields written directly under steps.<id> (e.g. Split's
        // stats/hasFailures, Switch's route) — label them by field name.
        fieldPath = param.path.slice(stepPrefix.length);
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
    'Current Item': [],
    'Iteration Context': [],
    'Loop Context': [],
    'Split Scope': [],
    'Wait Scope': [],
    'Workflow Inputs': [],
    Variables: [],
    'Step Outputs': [],
  };

  for (const suggestion of suggestions) {
    grouped[suggestion.group].push(suggestion);
  }

  return grouped;
}

export interface ConditionSuggestionContext {
  previousSteps: StepInfo[];
  inputSchemaFields?: SchemaField[];
  variables?: SimpleVariable[];
  isInsideWhileLoop?: boolean;
  isInsideSplit?: boolean;
  isInsideWaitScope?: boolean;
  /** Declared iteration schema of the enclosing Split, when inside one. */
  splitItemSchemaFields?: SchemaField[];
  /**
   * Include the per-element `item` scope — Filter conditions evaluate
   * against each array element via `item.*` references.
   */
  includeItemScope?: boolean;
}

/**
 * Suggestions for condition editors (Conditional/While/Filter conditions,
 * edge routes). Same canonical pipeline as the variable picker — the
 * condition editor used to carry a forked composer with hardcoded guessed
 * `item.*` field names (id, name, title, …) not driven by any schema.
 */
export function composeConditionSuggestions(
  context: ConditionSuggestionContext
): VariableSuggestion[] {
  const suggestions = composeVariableSuggestions(
    context.previousSteps,
    context.inputSchemaFields,
    context.variables,
    context.isInsideWhileLoop,
    context.isInsideSplit,
    context.isInsideWaitScope,
    context.splitItemSchemaFields
  );

  if (context.includeItemScope) {
    suggestions.unshift({
      label: 'item',
      value: 'item',
      description:
        'Current array element — reference its fields as item.<field>',
      group: 'Current Item',
    });
  }

  return suggestions;
}
