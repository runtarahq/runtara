import { WorkflowDto } from '@/generated/RuntaraRuntimeApi';
import { ExtendedAgent } from '@/features/workflows/queries';
import {
  getStepOutputShape,
  ShapeFieldJson,
} from '@/features/workflows/utils/step-output-shapes';
import { ExecutionGraph } from '../CustomNodes/utils.tsx';

export type ParameterType =
  | 'string'
  | 'number'
  | 'integer'
  | 'boolean'
  | 'array'
  | 'object'
  | 'null'
  | 'file';

export interface StepParameter {
  name: string;
  type?: ParameterType;
  path: string;
  children?: StepParameter[];
}

export interface StepInfo {
  id: string;
  name: string;
  inputs: StepParameter[];
  outputs: StepParameter[];
}

/**
 * Main function to compose information about previous steps
 */
export function composePreviousSteps({
  stepId,
  parentStepId,
  agents,
  executionGraph,
  workflows = [],
}: {
  stepId?: string;
  parentStepId?: string;
  agents: ExtendedAgent[];
  executionGraph: ExecutionGraph | null;
  workflows?: WorkflowDto[];
}): StepInfo[] {
  if (!stepId && !parentStepId) {
    throw new Error('Either stepId or parentStepId must be provided');
  }

  if (!executionGraph) {
    return []; // Return empty array if execution graph is null
  }

  // Special handling for nodes inside container steps (Split, While,
  // RepeatUntil, GroupBy — and WaitForSignal, whose nested graph is `onWait`)
  if (parentStepId) {
    // Find the parent container step
    const parentStep = executionGraph.steps?.[parentStepId];

    const containerGraph =
      parentStep?.subgraph ??
      (parentStep as { onWait?: ExecutionGraph } | undefined)?.onWait;
    if (containerGraph) {
      // We're inside a container - get previous steps from the container's subgraph
      const subgraph = containerGraph;

      // If stepId is provided, find steps before it in the subgraph
      // If stepId is not provided (creating new step), get all steps in subgraph
      let previousStepIds: string[] = [];

      if (stepId) {
        // Find direct parent steps within the subgraph
        const directParents = findParentStepIds(stepId, subgraph);

        // Get all ancestors of those parents
        if (directParents.length > 0) {
          previousStepIds = findPreviousSteps(directParents, subgraph);
        }
      } else {
        // No stepId means we're creating a new step - include all existing steps in container
        previousStepIds = Object.keys(subgraph.steps || {});
      }

      // Build StepInfo for steps inside the container
      // Only include sibling steps within the same subgraph - the DSL does not
      // allow referencing steps outside the container or in nested subgraphs
      return buildStepInfoList(previousStepIds, subgraph, agents, workflows);
    }
  }

  // Regular handling for steps not inside containers
  // Get parent step IDs (either from parameter or by finding them)
  let parentStepIds: string[] = [];
  if (parentStepId) {
    parentStepIds = [parentStepId];
  } else if (stepId) {
    parentStepIds = findParentStepIds(stepId, executionGraph);
  }

  if (parentStepIds.length === 0) {
    return []; // No parent steps found or provided
  }

  // Get all previous step IDs (including parents)
  const previousStepIds = findPreviousSteps(parentStepIds, executionGraph);

  // Build StepInfo for each previous step
  return buildStepInfoList(previousStepIds, executionGraph, agents, workflows);
}

/**
 * Builds StepInfo list for given step IDs from an execution graph
 */
function buildStepInfoList(
  stepIds: string[],
  executionGraph: ExecutionGraph,
  agents: ExtendedAgent[],
  workflows: WorkflowDto[]
): StepInfo[] {
  const result: StepInfo[] = [];

  for (const prevStepId of stepIds) {
    const step = executionGraph.steps?.[prevStepId];

    if (!step) continue;

    const inputs: StepParameter[] = [];
    const outputs: StepParameter[] = [];

    // Process inputMapping to create StepParameters
    for (const [inputKey] of Object.entries(step.inputMapping || {})) {
      let parameterType: ParameterType | undefined;
      const parameterSchema: any = null;

      // Calculate parameter type if step type is 'Agent'
      if (step.stepType === 'Agent' && step.agentId && step.capabilityId) {
        const agent = agents.find((a) => a.id === step.agentId);

        if (agent && agent.supportedCapabilities && step.capabilityId) {
          // Access capability directly by key
          const capability = agent.supportedCapabilities[step.capabilityId];

          if (capability && capability.inputs) {
            // Find the input field that matches inputKey
            const inputField = capability.inputs.find(
              (field) => field.name === inputKey
            );
            if (inputField) {
              parameterType = inputField.type as ParameterType;
            }
          }
        }
      }

      inputs.push(
        createStepParameter(
          inputKey,
          parameterType,
          prevStepId,
          parameterSchema
        )
      );
    }

    // Process output to create output StepParameters
    if (step.stepType === 'Agent' && step.agentId && step.capabilityId) {
      const agent = agents.find((a) => a.id === step.agentId);

      if (agent && agent.supportedCapabilities && step.capabilityId) {
        const capability = agent.supportedCapabilities[step.capabilityId];

        if (capability && capability.output) {
          const outputInfo = capability.output as any;

          if (outputInfo.fields && Array.isArray(outputInfo.fields)) {
            // Output has fields - show each field as a suggestion
            for (const field of outputInfo.fields) {
              outputs.push({
                name: field.name,
                type: field.type as ParameterType,
                path: `steps['${prevStepId}'].outputs.${field.name}`,
                children: undefined,
              });
            }
          } else {
            // Simple type output - show the outputs itself
            outputs.push({
              name: '',
              type: outputInfo.type as ParameterType,
              path: `steps['${prevStepId}'].outputs`,
              children:
                outputInfo.type === 'object' || outputInfo.type === 'array'
                  ? []
                  : undefined,
            });
          }
        }
      }
    } else if (step.stepType === 'EmbedWorkflow') {
      // For EmbedWorkflow steps, the outputs come from the child workflow

      // Try to find the child workflow and get its output schema
      let outputSchemaFound = false;
      const extendedStep = step as any;
      if (extendedStep.childWorkflowId && workflows.length > 0) {
        const childWorkflow = workflows.find(
          (s) => s.id === extendedStep.childWorkflowId
        );

        if (childWorkflow?.outputSchema) {
          try {
            const schemaString =
              typeof childWorkflow.outputSchema === 'string'
                ? childWorkflow.outputSchema
                : JSON.stringify(childWorkflow.outputSchema);

            const schema = JSON.parse(schemaString);

            if (schema.type === 'object' && schema.properties) {
              // Object with properties - show each property as a suggestion
              for (const [propName] of Object.entries<any>(schema.properties)) {
                const schemaInfo = parseJsonSchema(schemaString, propName);
                outputs.push(
                  createStepParameter(
                    propName,
                    schemaInfo.type,
                    prevStepId,
                    schemaInfo.schema
                  )
                );
              }
              outputSchemaFound = true;
            } else {
              // Simple type output
              outputs.push({
                name: '',
                type: schema.type as ParameterType,
                path: `steps['${prevStepId}'].outputs`,
                children:
                  schema.type === 'object' || schema.type === 'array'
                    ? []
                    : undefined,
              });
              outputSchemaFound = true;
            }
          } catch (e) {
            console.warn('Failed to parse child workflow output schema:', e);
          }
        }
      }

      // If we couldn't get the child workflow's output schema,
      // add a generic "outputs" parameter that users can reference
      if (!outputSchemaFound) {
        outputs.push({
          name: '',
          type: 'object',
          path: `steps['${prevStepId}'].outputs`,
          children: undefined,
        });
      }
    } else if (
      step.stepType === 'Wait' ||
      (step.stepType as string) === 'WaitForSignal'
    ) {
      // WaitForSignal steps output the signal response defined by responseSchema
      const waitStep = step as any;
      if (
        waitStep.responseSchema &&
        typeof waitStep.responseSchema === 'object'
      ) {
        for (const [fieldName, fieldDef] of Object.entries<any>(
          waitStep.responseSchema
        )) {
          const fieldType = fieldDef?.type as ParameterType | undefined;
          outputs.push({
            name: fieldName,
            type: isValidParameterType(fieldType ?? '')
              ? (fieldType as ParameterType)
              : 'string',
            path: `steps['${prevStepId}'].outputs.${fieldName}`,
            children: undefined,
          });
        }
      }
    } else {
      // Control-flow steps (Split, While, Filter, GroupBy, Conditional,
      // Switch, …): derive suggestions from the canonical per-step-type
      // output shape table (runtara-dsl step_output_shape via the validation
      // WASM) so they cannot drift from the runtime emitters. Hand-copied
      // shapes here drifted once already: While's iteration count was
      // suggested at steps['id'].iterations, which resolves to null at
      // runtime — the canonical path is steps['id'].outputs.iterations.
      appendShapeOutputs(step.stepType, prevStepId, outputs);
    }

    // Fallback: if no specific outputs were resolved (unknown step type or the
    // shape cache is still cold), add a generic outputs reference so the step
    // still appears in the variable picker
    if (outputs.length === 0) {
      outputs.push({
        name: '',
        type: 'object',
        path: `steps['${prevStepId}'].outputs`,
        children: undefined,
      });
    }

    result.push({
      id: prevStepId,
      name: step.name,
      inputs,
      outputs,
    });
  }

  return result;
}

/**
 * Maps a shape-field JSON type ("dynamic" included) to a ParameterType.
 * "dynamic" deliberately maps to undefined — the value's shape depends on
 * runtime data, so pretending to know its type would be a wrong badge.
 */
function shapeTypeToParameterType(type: string): ParameterType | undefined {
  return isValidParameterType(type) ? (type as ParameterType) : undefined;
}

/**
 * Appends output suggestions for a step type from the canonical output-shape
 * table (see utils/step-output-shapes.ts). Covers both the `outputs` value and
 * sibling fields written directly under `steps.<id>` (e.g. Split's
 * data/stats/hasFailures, Switch's route). No-op when the shape cache has not
 * been warmed yet — callers keep their generic fallback for that case.
 */
function appendShapeOutputs(
  stepType: string,
  stepId: string,
  outputs: StepParameter[]
): void {
  const shape = getStepOutputShape(stepType);
  if (!shape) {
    return;
  }

  const kind = shape.outputs?.kind;
  if (kind === 'object') {
    for (const field of shape.outputs?.fields ?? []) {
      outputs.push({
        name: field.name,
        type: shapeTypeToParameterType(field.type),
        path: `steps['${stepId}'].outputs.${field.name}`,
        children: undefined,
      });
    }
  } else if (kind === 'array') {
    outputs.push({
      name: '',
      type: 'array',
      path: `steps['${stepId}'].outputs`,
      children: undefined,
    });
  } else if (kind === 'dynamic') {
    outputs.push({
      name: '',
      type: undefined,
      path: `steps['${stepId}'].outputs`,
      children: undefined,
    });
  }

  for (const sibling of shape.siblingFields ?? []) {
    outputs.push(shapeSiblingParameter(sibling, stepId));
  }
}

function shapeSiblingParameter(
  sibling: ShapeFieldJson,
  stepId: string
): StepParameter {
  return {
    name: sibling.name,
    type: shapeTypeToParameterType(sibling.type),
    path: `steps['${stepId}'].${sibling.name}`,
    children: undefined,
  };
}

/**
 * Finds all parent step IDs for a given step ID
 */
function findParentStepIds(
  stepId: string,
  executionGraph: ExecutionGraph
): string[] {
  if (!executionGraph.executionPlan) return [];

  // Find all transitions where the toStep is our stepId
  return executionGraph.executionPlan
    .filter((transition) => transition.toStep === stepId)
    .map((transition) => transition.fromStep)
    .filter((step): step is string => !!step);
}

/**
 * Finds all previous steps from given parent steps
 * Returns steps ordered from nearest to farthest
 */
function findPreviousSteps(
  parentStepIds: string[],
  executionGraph: ExecutionGraph | null
): string[] {
  // Always include the parent steps themselves as previous steps
  if (!executionGraph || !executionGraph.executionPlan) {
    return [...parentStepIds];
  }

  // Start with the parent steps
  const result: string[] = [...parentStepIds];
  const visited = new Set<string>(parentStepIds);
  const queue: string[] = [...parentStepIds];

  while (queue.length > 0) {
    const currentStepId = queue.shift()!;

    // Find all incoming transitions to this step
    const incomingStepIds = executionGraph.executionPlan
      .filter((transition) => transition.toStep === currentStepId)
      .map((transition) => transition.fromStep)
      .filter((step): step is string => !!step);

    // Process each incoming step
    for (const prevStepId of incomingStepIds) {
      if (!visited.has(prevStepId)) {
        visited.add(prevStepId);
        result.push(prevStepId); // Add to result in order of discovery
        queue.push(prevStepId); // Continue BFS from this step
      }
    }
  }

  return result;
}

/**
 * Parse JSON schema and extract type information for a specific property path
 * Handles $defs and $ref references
 */
function parseJsonSchema(
  schemaString: string,
  propertyPath: string
): { type?: ParameterType; schema?: any } {
  try {
    const schema = JSON.parse(schemaString);

    // Helper function to resolve $ref
    function resolveRef(ref: string, rootSchema: any): any {
      if (!ref.startsWith('#/')) return null;

      const path = ref.substring(2).split('/');
      let current = rootSchema;

      for (const segment of path) {
        if (current[segment] === undefined) return null;
        current = current[segment];
      }

      return current;
    }

    // Function to find a property by its path (e.g., "body.headers")
    function findProperty(
      propPath: string,
      currentSchema: any
    ): { type?: ParameterType; schema?: any } {
      if (!currentSchema || !currentSchema.properties) return {};

      const parts = propPath.split('.');
      const firstPart = parts[0];

      // Handle array notation like body.messages[0].role
      const arrayMatch = firstPart.match(/^([^[]+)\[(\d+)\]$/);
      if (arrayMatch) {
        const arrayName = arrayMatch[1];
        const arrayProp = currentSchema.properties[arrayName];

        if (!arrayProp) return {};

        // Resolve any $ref in array property
        let resolvedArrayProp = arrayProp;
        if (arrayProp.$ref) {
          resolvedArrayProp = resolveRef(arrayProp.$ref, schema);
          if (!resolvedArrayProp) return {};
        }

        // Check if it's an array with items
        if (resolvedArrayProp.type === 'array' && resolvedArrayProp.items) {
          let itemSchema = resolvedArrayProp.items;

          // Resolve item schema if it's a reference
          if (itemSchema.$ref) {
            itemSchema = resolveRef(itemSchema.$ref, schema);
            if (!itemSchema) return {};
          }

          // Continue with the rest of the path
          if (parts.length > 1) {
            return findProperty(parts.slice(1).join('.'), {
              properties: itemSchema,
            });
          }

          return {
            type: itemSchema.type as ParameterType,
            schema: itemSchema,
          };
        }

        return {};
      }

      // Regular property lookup
      let property = currentSchema.properties[firstPart];

      // Handle property not found
      if (!property) return {};

      // Resolve $ref if needed
      if (property.$ref) {
        property = resolveRef(property.$ref, schema);
        if (!property) return {};
      }

      // If this is a nested path, continue to the next part
      if (parts.length > 1) {
        return findProperty(parts.slice(1).join('.'), property);
      }

      // Return the property type
      return {
        type: property.type as ParameterType,
        schema: property,
      };
    }

    return findProperty(propertyPath, schema);
  } catch (error) {
    console.error('Error parsing schema:', error);
    return {};
  }
}

/**
 * Creates a StepParameter with potential child parameters for complex types
 */
function createStepParameter(
  name: string,
  type: ParameterType | undefined,
  stepId: string,
  schema: any = null
): StepParameter {
  // When referencing previous steps, use .outputs instead of .inputs
  const path = `steps['${stepId}'].outputs.${name}`;

  const parameter: StepParameter = {
    name,
    type,
    path,
  };

  // Add children for array or object types
  if ((type === 'array' || type === 'object') && schema) {
    const children: StepParameter[] = [];

    if (type === 'object' && schema.properties) {
      // Process object properties
      for (const [childName, childSchema] of Object.entries(
        schema.properties
      )) {
        const childType = (childSchema as any).type as ParameterType;

        if (isValidParameterType(childType)) {
          children.push(
            createStepParameter(
              `${name}.${childName}`,
              childType,
              stepId,
              childSchema
            )
          );
        } else if ((childSchema as any).$ref) {
          // Handle reference properties
          children.push(
            createStepParameter(`${name}.${childName}`, 'object', stepId)
          );
        }
      }
    } else if (type === 'array' && schema.items) {
      // Process array items
      if (Array.isArray(schema.items)) {
        // Handle tuple type
        schema.items.forEach((itemSchema: any, index: number) => {
          const itemType = itemSchema.type as ParameterType;
          if (isValidParameterType(itemType)) {
            children.push(
              createStepParameter(
                `${name}[${index}]`,
                itemType,
                stepId,
                itemSchema
              )
            );
          }
        });
      } else {
        // Handle single item schema
        const itemType = schema.items.type as ParameterType;
        if (isValidParameterType(itemType)) {
          children.push(
            createStepParameter(`${name}.item`, itemType, stepId, schema.items)
          );
        } else if (schema.items.$ref) {
          // Handle reference item type
          children.push(createStepParameter(`${name}.item`, 'object', stepId));
        }
      }
    }

    if (children.length > 0) {
      parameter.children = children;
    }
  }

  return parameter;
}

/**
 * Checks if a type string is a valid ParameterType
 */
function isValidParameterType(type: string): boolean {
  const validTypes: ParameterType[] = [
    'string',
    'number',
    'integer',
    'boolean',
    'array',
    'object',
    'null',
    'file',
  ];
  return validTypes.includes(type as ParameterType);
}
