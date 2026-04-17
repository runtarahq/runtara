import type {
  ExecutionGraphDto,
  ExecutionGraphStepDto,
} from '@/features/scenarios/types/execution-graph';

/**
 * Migration result containing the migrated graph and extracted data
 */
export interface MigrationResult {
  /** The migrated execution graph without Start step */
  executionGraph: ExecutionGraphDto;
  /** Whether migration was performed */
  wasMigrated: boolean;
  /** Input schema extracted from Start step (if any) */
  extractedInputSchema?: Record<string, any>;
  /** Variables extracted from Start step (if any) */
  extractedVariables?: Record<string, any>;
}

/**
 * Migrates a legacy execution graph that contains a Start step.
 *
 * The migration:
 * 1. Finds the Start step in the graph
 * 2. Extracts inputSchema and variables from the Start step
 * 3. Removes the Start step from the graph
 * 4. Updates edges to bypass the Start step
 * 5. Updates entry point to the first real step
 *
 * @param executionGraph - The original execution graph (may contain Start step)
 * @returns Migration result with cleaned graph and extracted data
 */
export function migrateStartStep(
  executionGraph: ExecutionGraphDto
): MigrationResult {
  const {
    steps = {},
    executionPlan = [],
    entryPoint,
    ...rest
  } = executionGraph;

  // Find Start step(s) in the graph
  const startStepEntries = Object.entries(steps).filter(
    ([, step]) => step.stepType === 'Start'
  );

  // If no Start step, return original graph unchanged
  if (startStepEntries.length === 0) {
    return {
      executionGraph,
      wasMigrated: false,
    };
  }

  // Get the first Start step (there should only be one)
  const [startStepId, startStep] = startStepEntries[0];

  // Extract schema and variables from Start step's inputMapping
  let extractedInputSchema: Record<string, any> | undefined;
  let extractedVariables: Record<string, any> | undefined;

  const startInputMapping = startStep.inputMapping as any;

  // Check if Start step has inputSchema directly
  if ((startStep as any).inputSchema) {
    extractedInputSchema = (startStep as any).inputSchema;
  }

  // Check for structured format with data and variables sections
  if (startInputMapping) {
    if (startInputMapping.data && typeof startInputMapping.data === 'object') {
      // New structured format - data section maps to inputSchema
      // We'd need to infer schema from the data mappings
      // For now, use inputSchema if available
    }

    if (
      startInputMapping.variables &&
      typeof startInputMapping.variables === 'object'
    ) {
      // Extract variables
      extractedVariables = {};
      for (const [varName, varDef] of Object.entries(
        startInputMapping.variables
      )) {
        if (
          typeof varDef === 'object' &&
          varDef !== null &&
          'value' in (varDef as any)
        ) {
          extractedVariables[varName] = (varDef as any).value;
        } else {
          extractedVariables[varName] = varDef;
        }
      }
    }
  }

  // Find edges from Start step to determine the first real step
  const edgesFromStart = executionPlan.filter(
    (edge) => edge.fromStep === startStepId
  );

  // Get the first step after Start (this becomes the new entry point)
  const firstRealStepId =
    edgesFromStart.length > 0 ? edgesFromStart[0].toStep : undefined;

  // Remove Start step from steps
  const migratedSteps: Record<string, ExecutionGraphStepDto> = {};
  for (const [id, step] of Object.entries(steps)) {
    if (id !== startStepId) {
      migratedSteps[id] = step;
    }
  }

  // Update execution plan to remove edges from/to Start step
  // Also update edges that were going TO Start step to go to the first real step
  const migratedExecutionPlan = executionPlan
    .filter(
      (edge) => edge.fromStep !== startStepId && edge.toStep !== startStepId
    )
    .map((edge) => {
      // If any edge was pointing to Start, redirect to first real step
      // (This shouldn't happen in normal workflows, but handle it just in case)
      if (edge.toStep === startStepId && firstRealStepId) {
        return { ...edge, toStep: firstRealStepId };
      }
      return edge;
    });

  // Determine new entry point
  // If the original entry point was the Start step, use the first real step
  // Otherwise, keep the original entry point
  const newEntryPoint =
    entryPoint === startStepId ? firstRealStepId : entryPoint;

  const migratedGraph: ExecutionGraphDto = {
    ...rest,
    steps: migratedSteps,
    executionPlan: migratedExecutionPlan,
    entryPoint: newEntryPoint,
  };

  return {
    executionGraph: migratedGraph,
    wasMigrated: true,
    extractedInputSchema,
    extractedVariables,
  };
}

/**
 * Checks if an execution graph contains a Start step and needs migration.
 *
 * @param executionGraph - The execution graph to check
 * @returns true if the graph contains a Start step
 */
export function needsStartStepMigration(
  executionGraph: ExecutionGraphDto
): boolean {
  const { steps = {} } = executionGraph;
  return Object.values(steps).some((step) => step.stepType === 'Start');
}
