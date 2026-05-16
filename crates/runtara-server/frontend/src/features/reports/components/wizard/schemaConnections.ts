import { Schema } from '@/generated/RuntaraRuntimeApi';

export type SchemasByConnectionId = Record<string, Schema[]>;

export function schemasForConnection(
  fallbackSchemas: Schema[],
  schemasByConnectionId: SchemasByConnectionId | undefined,
  connectionId: string | null | undefined,
  defaultConnectionId: string | null | undefined
): Schema[] {
  const resolvedConnectionId = connectionId ?? defaultConnectionId;
  if (!resolvedConnectionId) return fallbackSchemas;
  return schemasByConnectionId?.[resolvedConnectionId] ?? fallbackSchemas;
}

export function fieldsOfSchema(
  schemas: Schema[],
  schemaName: string | undefined
): string[] {
  if (!schemaName) return [];
  return (
    schemas
      .find((schema) => schema.name === schemaName)
      ?.columns.map((column) => column.name) ?? []
  );
}

export function schemaFieldsByName(
  schemas: Schema[]
): Record<string, string[]> {
  return Object.fromEntries(
    schemas.map((schema) => [
      schema.name,
      schema.columns.map((column) => column.name),
    ])
  );
}
