import { UIVariable } from './VariablesEditor';
import { SchemaField } from './SchemaFieldsEditor';
import type { MemoryTier } from '@/generated/RuntaraRuntimeApi';

export interface WorkflowData {
  id: string;
  name: string;
  description?: string;
  variables?: UIVariable[];
  inputSchemaFields?: SchemaField[];
  outputSchemaFields?: SchemaField[];
  executionTimeoutSeconds?: number;
  rateLimitBudgetMs?: number;
  durable?: boolean | null;
  entryPoint?: string;
  memoryTier?: MemoryTier | null;
  trackEvents?: boolean;
}
