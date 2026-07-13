import { WorkflowDto, StepTypeInfo } from '@/generated/RuntaraRuntimeApi';
import { ExtendedAgent } from '@/features/workflows/queries';
import { createContext } from 'react';
import { ExecutionGraph } from '../CustomNodes/utils.tsx';
import { StepInfo } from './shared.ts';
import { SchemaField } from '../EditorSidebar/SchemaFieldsEditor';

/** Simple variable type matching the WorkflowEditor prop type */
export interface SimpleVariable {
  name: string;
  value: unknown;
  type: string;
  description?: string | null;
}

export interface NodeFormContextContextData {
  nodeId?: string;
  parentNodeId?: string;
  stepTypes: StepTypeInfo[];
  agents: ExtendedAgent[];
  workflows: WorkflowDto[];
  executionGraph: ExecutionGraph | null;
  isLoading: boolean;
  previousSteps: StepInfo[];
  outputSchemaFields?: SchemaField[];
  /** Workflow input schema fields for variable suggestions */
  inputSchemaFields?: SchemaField[];
  /** Workflow variables (constants) for variable suggestions */
  variables?: SimpleVariable[];
  /** Whether this step is inside a While loop (or the While condition itself) */
  isInsideWhileLoop?: boolean;
  /** Whether this step is inside a Split iteration subgraph */
  isInsideSplit?: boolean;
  /** Whether this step is inside a WaitForSignal onWait subgraph */
  isInsideWaitScope?: boolean;
  /**
   * The enclosing Split's declared iteration schema (its per-item
   * inputSchema), when this step is inside a Split subgraph. Feeds data.*
   * suggestions and reference-type resolution in that scope.
   */
  splitItemSchemaFields?: SchemaField[];
}

export const NodeFormContext = createContext<NodeFormContextContextData>({
  stepTypes: [],
  agents: [],
  workflows: [],
  executionGraph: null,
  isLoading: false,
  previousSteps: [],
} as NodeFormContextContextData);
