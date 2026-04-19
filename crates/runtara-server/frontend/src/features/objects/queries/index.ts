import {
  Condition,
  CreateInstanceRequest,
  CreateSchemaRequest,
  UpdateInstanceRequest,
  UpdateSchemaRequest,
  BulkDeleteRequest,
  CsvExportRequest,
  CsvImportJsonRequest,
  CsvPreviewJsonRequest,
  CsvImportResponse,
  ImportPreviewResponse,
} from '@/generated/RuntaraRuntimeApi';
import { RuntimeREST } from '@/shared/queries';
import { createAuthHeaders } from '@/shared/queries/utils';

/**
 * Recursively checks whether a filter condition is complete
 * (all field names and values are non-empty).
 * Returns false for incomplete conditions so they are not sent to the backend.
 */
function isConditionComplete(condition: Condition | null | undefined): boolean {
  if (!condition || !condition.op) return false;

  const args = condition.arguments;
  if (!args || args.length === 0) return false;

  const op = condition.op.toUpperCase();

  // Logical operators: all nested sub-conditions must be complete
  if (op === 'AND' || op === 'OR' || op === 'NOT') {
    return args.every((arg: any) => {
      if (typeof arg === 'object' && arg !== null && 'op' in arg) {
        return isConditionComplete(arg);
      }
      return false;
    });
  }

  // Unary operators (IS_EMPTY, IS_NOT_EMPTY, IS_DEFINED): field name must be non-empty
  if (op === 'IS_EMPTY' || op === 'IS_NOT_EMPTY' || op === 'IS_DEFINED') {
    return typeof args[0] === 'string' && args[0].trim() !== '';
  }

  // Binary operators (EQ, NE, GT, etc.): field name and value must be non-empty
  if (args.length >= 2) {
    const field = args[0];
    const value = args[1];

    if (typeof field !== 'string' || field.trim() === '') return false;

    // Value can be a nested condition
    if (typeof value === 'object' && value !== null && 'op' in value) {
      return isConditionComplete(value);
    }

    // Value must be a non-empty string/number/boolean
    if (typeof value === 'string') return value.trim() !== '';
    if (typeof value === 'number' || typeof value === 'boolean') return true;

    return false;
  }

  return false;
}

// Schema related queries
export async function getAllSchemas(token: string) {
  const result = await RuntimeREST.api.listSchemas(
    {
      offset: 0,
      limit: 1000,
    },
    createAuthHeaders(token)
  );

  return result.data.schemas || [];
}

export async function getSchemaById(token: string, context: any) {
  // Query key structure: ['objects', 'schemas', 'detail', id]
  const [, , , id] = context.queryKey;

  if (!id) return null;

  const result = await RuntimeREST.api.getSchemaById(
    id,
    {},
    createAuthHeaders(token)
  );

  return result.data.schema || null;
}

export async function createSchema(token: string, schema: CreateSchemaRequest) {
  try {
    const result = await RuntimeREST.api.createSchema(
      schema,
      {},
      createAuthHeaders(token)
    );

    // Return the created schema ID in a format compatible with the old API
    // We need to fetch the schema to return the full object
    if (result.data.success && result.data.schemaId) {
      const schemaResult = await RuntimeREST.api.getSchemaById(
        result.data.schemaId,
        {},
        createAuthHeaders(token)
      );
      return schemaResult.data.schema;
    }

    throw new Error(result.data.message || 'Failed to create schema');
  } catch (error) {
    // Rethrow with user-friendly message for duplicate schema name errors
    const err = error as {
      response?: { status?: number; data?: { message?: string } };
    };
    if (err.response?.status === 409) {
      throw Object.assign(new Error('Schema with this name already exists'), {
        response: err.response,
      });
    }
    throw error;
  }
}

export async function updateSchema(
  token: string,
  id: string,
  data: UpdateSchemaRequest
) {
  try {
    const result = await RuntimeREST.api.updateSchema(
      id,
      data,
      {},
      createAuthHeaders(token)
    );

    // Fetch the updated schema to return the full object
    if (result.data.success) {
      const schemaResult = await RuntimeREST.api.getSchemaById(
        id,
        {},
        createAuthHeaders(token)
      );
      return schemaResult.data.schema;
    }

    throw new Error(result.data.message || 'Failed to update schema');
  } catch (error) {
    // Rethrow with user-friendly message for duplicate schema name errors
    const err = error as {
      response?: { status?: number; data?: { message?: string } };
    };
    if (err.response?.status === 409) {
      throw Object.assign(new Error('Schema with this name already exists'), {
        response: err.response,
      });
    }
    throw error;
  }
}

export async function deleteSchema(token: string, id: string) {
  await RuntimeREST.api.deleteSchema(id, {}, createAuthHeaders(token));
}

// Instance related queries
export async function getInstancesBySchema(token: string, context: any) {
  // Query key structure: ['objects', 'instances', schemaId, params]
  const [, , schemaId, params = {}] = context.queryKey;
  const {
    page = 0,
    size = 20,
    condition = null,
    schemaName = null,
    sortBy = undefined,
    sortOrder = undefined,
  } = params as {
    page?: number;
    size?: number;
    condition?: any;
    schemaName?: string | null;
    sortBy?: string[];
    sortOrder?: string[];
  };

  if (!schemaId)
    return { content: [], totalPages: 0, totalElements: 0, number: 0 };

  const offset = page * size;

  // Check if filtering or sorting is requested but schemaName is missing
  const hasFilteringOrSorting = condition || sortBy || sortOrder;

  if (hasFilteringOrSorting && !schemaName) {
    console.error(
      '[ObjectModel] Schema name is required for filtering/sorting but was not provided.',
      { schemaId, condition, sortBy, sortOrder }
    );
    throw new Error(
      'Schema name is required to apply filters or sorting. Please ensure the schema has a name defined.'
    );
  }

  // Use filterInstances endpoint when we have schemaName (supports sorting and filtering)
  if (schemaName) {
    const result = await RuntimeREST.api.filterInstances(
      schemaName,
      {
        condition: isConditionComplete(condition) ? condition : undefined,
        offset,
        limit: size,
        sortBy,
        sortOrder,
      },
      {},
      createAuthHeaders(token)
    );

    const data = result.data;
    const totalElements = data.totalCount || 0;
    const totalPages = size > 0 ? Math.ceil(totalElements / size) : 0;

    return {
      content: data.instances || [],
      totalPages,
      totalElements,
      number: page,
    };
  }

  // Fallback to regular list endpoint (no filtering/sorting support)
  // This path is only used when no schemaName is available AND no filters/sorting requested
  const result = await RuntimeREST.api.getInstancesBySchema(
    schemaId,
    {
      offset,
      limit: size,
    },
    createAuthHeaders(token)
  );

  const data = result.data;
  const totalElements = data.totalCount || 0;
  const totalPages = size > 0 ? Math.ceil(totalElements / size) : 0;

  return {
    content: data.instances || [],
    totalPages,
    totalElements,
    number: page,
  };
}

export async function getInstanceById(token: string, context: any) {
  // Query key structure: ['objects', 'instances', schemaId, instanceId]
  const [, , schemaId, instanceId] = context.queryKey;

  if (!schemaId || !instanceId) return null;

  const result = await RuntimeREST.api.getInstanceById(
    schemaId,
    instanceId,
    {},
    createAuthHeaders(token)
  );

  return result.data.instance || null;
}

export async function createInstance(
  token: string,
  data: CreateInstanceRequest
) {
  const result = await RuntimeREST.api.createInstance(
    data,
    {},
    createAuthHeaders(token)
  );

  // Fetch the created instance to return the full object
  if (result.data.success && result.data.instanceId && data.schemaId) {
    const instanceResult = await RuntimeREST.api.getInstanceById(
      data.schemaId,
      result.data.instanceId,
      {},
      createAuthHeaders(token)
    );
    return instanceResult.data.instance;
  }

  throw new Error(result.data.message || 'Failed to create instance');
}

export async function updateInstance(
  token: string,
  schemaId: string,
  instanceId: string,
  data: UpdateInstanceRequest
) {
  const result = await RuntimeREST.api.updateInstance(
    schemaId,
    instanceId,
    data,
    {},
    createAuthHeaders(token)
  );

  // Fetch the updated instance to return the full object
  if (result.data.success) {
    const instanceResult = await RuntimeREST.api.getInstanceById(
      schemaId,
      instanceId,
      {},
      createAuthHeaders(token)
    );
    return instanceResult.data.instance;
  }

  throw new Error(result.data.message || 'Failed to update instance');
}

export async function bulkDeleteInstances(
  token: string,
  schemaId: string,
  instanceIds: string[]
) {
  const requestData: BulkDeleteRequest = {
    instanceIds,
  };

  const result = await RuntimeREST.api.bulkDeleteInstances(
    schemaId,
    requestData,
    {},
    createAuthHeaders(token)
  );

  return result.data.deletedCount || 0;
}

export type BulkConflictMode = 'error' | 'skip' | 'upsert';
export type BulkValidationMode = 'stop' | 'skip';

export interface BulkCreateOptions {
  onConflict: BulkConflictMode;
  onError: BulkValidationMode;
  conflictColumns: string[];
}

export interface BulkCreateResult {
  success: boolean;
  createdCount: number;
  skippedCount: number;
  errors: Array<{ index: number; reason: string }>;
  message?: string;
}

/**
 * Bulk-insert records via POST /instances/{schema_id}/bulk with opt-in conflict
 * and validation handling.
 */
export async function bulkCreateInstances(
  token: string,
  schemaId: string,
  instances: unknown[],
  opts: BulkCreateOptions
): Promise<BulkCreateResult> {
  const result = await RuntimeREST.instance.post(
    `/api/runtime/object-model/instances/${schemaId}/bulk`,
    {
      instances,
      onConflict: opts.onConflict,
      onError: opts.onError,
      conflictColumns: opts.conflictColumns,
    },
    { headers: { Authorization: `Bearer ${token}` } }
  );
  return {
    success: !!result.data?.success,
    createdCount: result.data?.createdCount ?? 0,
    skippedCount: result.data?.skippedCount ?? 0,
    errors: result.data?.errors ?? [],
    message: result.data?.message,
  };
}

/**
 * Bulk-update instances by applying the same `properties` to every row whose id
 * is in `instanceIds`. Uses the generic PATCH /instances/{schema_id}/bulk endpoint
 * with `mode: "byCondition"` and an IN(id, [...]) condition built from the
 * selected rows.
 */
export async function bulkUpdateInstancesByIds(
  token: string,
  schemaId: string,
  instanceIds: string[],
  properties: Record<string, unknown>
): Promise<number> {
  const result = await RuntimeREST.instance.patch(
    `/api/runtime/object-model/instances/${schemaId}/bulk`,
    {
      mode: 'byCondition',
      condition: { op: 'IN', arguments: ['id', instanceIds] },
      properties,
    },
    { headers: { Authorization: `Bearer ${token}` } }
  );
  return result.data?.updatedCount ?? 0;
}

// CSV Import/Export functions

export async function exportCsv(
  token: string,
  schemaName: string,
  data: CsvExportRequest
): Promise<Blob> {
  const result = await RuntimeREST.instance.post(
    `/api/runtime/object-model/instances/schema/${schemaName}/export-csv`,
    data,
    {
      headers: { Authorization: `Bearer ${token}` },
      responseType: 'blob',
    }
  );
  return result.data;
}

export async function importCsvPreview(
  token: string,
  schemaName: string,
  data: CsvPreviewJsonRequest
): Promise<ImportPreviewResponse> {
  const result = await RuntimeREST.instance.post(
    `/api/runtime/object-model/instances/schema/${schemaName}/import-csv/preview`,
    data,
    {
      headers: {
        Authorization: `Bearer ${token}`,
        'Content-Type': 'application/json',
      },
    }
  );
  return result.data;
}

export async function importCsv(
  token: string,
  schemaName: string,
  data: CsvImportJsonRequest
): Promise<CsvImportResponse> {
  const result = await RuntimeREST.instance.post(
    `/api/runtime/object-model/instances/schema/${schemaName}/import-csv`,
    data,
    {
      headers: {
        Authorization: `Bearer ${token}`,
        'Content-Type': 'application/json',
      },
    }
  );
  return result.data;
}
