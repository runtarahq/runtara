/**
 * Local type definitions for ExecutionGraph structures.
 * These define the internal structure of scenario execution graphs
 * used for workflow editing and manipulation.
 */

export interface ExecutionGraphDto {
  /** Scenario name (required for updates) */
  name?: string;
  /** Scenario description (optional) */
  description?: string;
  steps?: Record<string, ExecutionGraphStepDto>;
  executionPlan?: ExecutionGraphTransitionDto[];
  entryPoint?: string;
  /** Variables with their types and default values */
  variables?: Record<string, { type: string; value: string }>;
  /** JSON Schema for scenario input validation */
  inputSchema?: Record<string, unknown>;
  /** JSON Schema for scenario output validation */
  outputSchema?: Record<string, unknown>;
  /** Execution timeout in seconds (default: 300, max: 3600) */
  executionTimeoutSeconds?: number;
  /** Rate limit wait budget in milliseconds (default: 60000, min: 1000, max: 86400000) */
  rateLimitBudgetMs?: number;
}

/** Mapping value structure for inputMapping and config fields */
export interface MappingValue {
  valueType: 'reference' | 'immediate' | 'composite' | 'template';
  value: unknown;
  type?: string;
}

/** Configuration for Split step */
export interface SplitStepConfig {
  /** The array to iterate over */
  value: MappingValue;
  /** Max concurrent iterations (0 = unlimited) */
  parallelism?: number;
  /** Execute iterations sequentially */
  sequential?: boolean;
  /** Continue even if some iterations fail */
  dontStopOnFailed?: boolean;
  /** Additional variables to pass to each iteration's subgraph */
  variables?: Record<string, MappingValue>;
}

export interface ExecutionGraphStepDto {
  id?: string;
  name?: string;
  description?: string;
  agentId?: string;
  connectionId?: string;
  capabilityId?: string;
  /** When true, execution pauses before this step in debug mode */
  breakpoint?: boolean;
  stepType?:
    | 'Start'
    | 'Agent'
    | 'GroupBy'
    | 'Conditional'
    | 'StartScenario'
    | 'Finish'
    | 'Split'
    | 'Combine'
    | 'Wait'
    | 'Event'
    | 'RepeatUntil'
    | 'Filter'
    | 'While';
  inputMapping?: Record<string, string>;
  /** Configuration for Split steps (replaces inputMapping for Split) */
  config?: SplitStepConfig;
  links?: string[];
  /** @format int64 */
  executionTimeout?: number;
  /** @format int32 */
  maxRetries?: number;
  /** @format int64 */
  retryDelay?: number;
  retryStrategy?: 'Linear' | 'Exponential';
  renderingParameters?: ExecutionGraphStepRenderingParametersDto;
  subgraph?: ExecutionGraphDto;
}

export interface ExecutionGraphStepRenderingParametersDto {
  /** @format double */
  x?: number;
  /** @format double */
  y?: number;
  /** @format double */
  width?: number;
  /** @format double */
  height?: number;
}

export interface ExecutionGraphTransitionDto {
  fromStep?: string;
  toStep?: string;
  label?: string;
}
