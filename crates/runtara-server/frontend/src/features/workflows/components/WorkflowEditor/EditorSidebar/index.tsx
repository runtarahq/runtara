import { UIVariable } from './VariablesEditor';
import { SchemaField } from './SchemaFieldsEditor';

export interface WorkflowData {
  id: string;
  name: string;
  description?: string;
  variables?: UIVariable[];
  inputSchemaFields?: SchemaField[];
  outputSchemaFields?: SchemaField[];
  executionTimeoutSeconds?: number;
  rateLimitBudgetMs?: number;
}
