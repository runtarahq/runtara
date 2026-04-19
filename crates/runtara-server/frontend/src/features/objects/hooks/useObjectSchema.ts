import { Schema } from '@/generated/RuntaraRuntimeApi';
import { useObjectSchemaDtos } from './useObjectSchemas';

// Get a single object type by name, derived from the cached schemas list
// so it inherits the correct auth gate and cache invalidation.
export function useObjectSchemaDto(typeName: string | undefined) {
  const { data: schemas, isLoading, error } = useObjectSchemaDtos();

  const schema: Schema | null =
    typeName && schemas
      ? (schemas.find((s) => s.name === typeName) ?? null)
      : null;

  return { data: schema, isLoading, error };
}
