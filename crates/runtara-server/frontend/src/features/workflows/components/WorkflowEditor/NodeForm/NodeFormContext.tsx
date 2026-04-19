import { WorkflowDto, StepTypeInfo } from '@/generated/RuntaraRuntimeApi';
import { ExtendedAgent } from '@/features/workflows/queries';
import { createContext } from 'react';
import { ExecutionGraph } from '../CustomNodes/utils.tsx';
import { StepInfo } from './shared.ts';
import { SchemaField } from '../EditorSidebar/SchemaFieldsEditor';

/** Simple variable type matching the WorkflowEditor prop type */
export interface SimpleVariable {
  name: string;
  value: string;
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
}

export const NodeFormContext = createContext<NodeFormContextContextData>({
  stepTypes: [],
  agents: [],
  workflows: [],
  executionGraph: null,
  isLoading: false,
  previousSteps: [],
} as NodeFormContextContextData);
