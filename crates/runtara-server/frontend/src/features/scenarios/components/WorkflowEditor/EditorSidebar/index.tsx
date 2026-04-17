import { UIVariable } from './VariablesEditor';
import { SchemaField } from './SchemaFieldsEditor';

export interface ScenarioData {
  id: string;
  name: string;
  description?: string;
  variables?: UIVariable[];
  inputSchemaFields?: SchemaField[];
  outputSchemaFields?: SchemaField[];
  executionTimeoutSeconds?: number;
  rateLimitBudgetMs?: number;
}
