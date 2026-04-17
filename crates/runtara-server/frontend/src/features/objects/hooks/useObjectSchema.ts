import { useQuery } from '@tanstack/react-query';
import { getAllSchemas } from '../queries';
import { Schema } from '@/generated/RuntaraRuntimeApi';
import { useToken } from '@/shared/hooks';

// Query key for object types
const objectSchemaDtosKey = ['objectSchemaDtos'];

// Get a single object type by name
export function useObjectSchemaDto(typeName: string | undefined) {
  const token = useToken();

  return useQuery<Schema | null>({
    queryKey: [...objectSchemaDtosKey, 'byName', typeName],
    queryFn: async () => {
      if (!typeName) return null;

      // Get all schemas and find the one with the matching name
      const schemas = await getAllSchemas(token);
      const schema = schemas.find((schema) => schema.name === typeName);

      if (!schema) return null;

      return schema;
    },
    enabled: !!typeName && !!token,
  });
}
