import { UIVariable } from './VariablesEditor';
import { SchemaField } from './SchemaFieldsEditor';

export interface WorkflowData {
  id: string;
  name: string;
  description?: string;
  variables?: UIVariable[];
  inputSchemaFields?: SchemaField[];
  outputSchemaFields?: SchemaField[];
  stateSchemaFields?: SchemaField[];
  executionTimeoutSeconds?: number;
  rateLimitBudgetMs?: number;
  durable?: boolean | null;
  entryPoint?: string;
  entryPointOptions?: Array<{ id: string; name: string }>;
  memoryMb?: number | null;
  trackEvents?: boolean;
}
