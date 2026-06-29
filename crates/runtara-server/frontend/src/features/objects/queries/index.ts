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

const queryWithConnection = (connectionId?: string | null) =>
  connectionId ? { connectionId } : {};

const appendConnectionId = (url: string, connectionId?: string | null) => {
  if (!connectionId) return url;
  const separator = url.includes('?') ? '&' : '?';
  return `${url}${separator}connectionId=${encodeURIComponent(connectionId)}`;
};

// Schema related queries
export async function getAllSchemas(
  token: string,
  connectionId?: string | null
) {
  const result = await RuntimeREST.api.listSchemas(
    {
      offset: 0,
      limit: 1000,
      ...queryWithConnection(connectionId),
    },
    createAuthHeaders(token)
  );

  return result.data.schemas || [];
}

export async function getSchemaById(token: string, context: any) {
  const queryKey = context.queryKey as unknown[];
  const id = queryKey[queryKey.length - 1] as string | undefined;
  const connectionId = queryKey[2] as string | null | undefined;

  if (!id) return null;

  const result = await RuntimeREST.api.getSchemaById(
    id,
    queryWithConnection(connectionId),
    createAuthHeaders(token)
  );

  return result.data.schema || null;
}

export async function updateSchema(
  token: string,
  id: string,
  data: UpdateSchemaRequest,
  connectionId?: string | null
) {
  try {
    const result = await RuntimeREST.api.updateSchema(
      id,
      data,
      queryWithConnection(connectionId),
      createAuthHeaders(token)
    );

    // Fetch the updated schema to return the full object
    if (result.data.success) {
      const schemaResult = await RuntimeREST.api.getSchemaById(
        id,
        queryWithConnection(connectionId),
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

export async function createSchemaWithConnection(
  token: string,
  schema: CreateSchemaRequest,
  connectionId?: string | null
) {
  try {
    const result = await RuntimeREST.api.createSchema(
      schema,
      queryWithConnection(connectionId),
      createAuthHeaders(token)
    );

    if (result.data.success && result.data.schemaId) {
      const schemaResult = await RuntimeREST.api.getSchemaById(
        result.data.schemaId,
        queryWithConnection(connectionId),
        createAuthHeaders(token)
      );
      return schemaResult.data.schema;
    }

    throw new Error(result.data.message || 'Failed to create schema');
  } catch (error) {
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

export async function deleteSchema(
  token: string,
  id: string,
  connectionId?: string | null
) {
  await RuntimeREST.api.deleteSchema(
    id,
    queryWithConnection(connectionId),
    createAuthHeaders(token)
  );
}

// Instance related queries
export async function getInstancesBySchema(token: string, context: any) {
  const [, , connectionId, schemaId, params = {}] = context.queryKey;
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
      queryWithConnection(connectionId),
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
      ...queryWithConnection(connectionId),
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
  const [, , connectionId, schemaId, instanceId] = context.queryKey;

  if (!schemaId || !instanceId) return null;

  const result = await RuntimeREST.api.getInstanceById(
    schemaId,
    instanceId,
    queryWithConnection(connectionId),
    createAuthHeaders(token)
  );

  return result.data.instance || null;
}

export async function createInstance(
  token: string,
  data: CreateInstanceRequest,
  connectionId?: string | null
) {
  const result = await RuntimeREST.api.createInstance(
    data,
    queryWithConnection(connectionId),
    createAuthHeaders(token)
  );

  // Fetch the created instance to return the full object
  if (result.data.success && result.data.instanceId && data.schemaId) {
    const instanceResult = await RuntimeREST.api.getInstanceById(
      data.schemaId,
      result.data.instanceId,
      queryWithConnection(connectionId),
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
  data: UpdateInstanceRequest,
  connectionId?: string | null
) {
  const result = await RuntimeREST.api.updateInstance(
    schemaId,
    instanceId,
    data,
    queryWithConnection(connectionId),
    createAuthHeaders(token)
  );

  // Fetch the updated instance to return the full object
  if (result.data.success) {
    const instanceResult = await RuntimeREST.api.getInstanceById(
      schemaId,
      instanceId,
      queryWithConnection(connectionId),
      createAuthHeaders(token)
    );
    return instanceResult.data.instance;
  }

  throw new Error(result.data.message || 'Failed to update instance');
}

export async function bulkDeleteInstances(
  token: string,
  schemaId: string,
  instanceIds: string[],
  connectionId?: string | null
) {
  const requestData: BulkDeleteRequest = {
    instanceIds,
  };

  const result = await RuntimeREST.api.bulkDeleteInstances(
    schemaId,
    requestData,
    queryWithConnection(connectionId),
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
  opts: BulkCreateOptions,
  connectionId?: string | null
): Promise<BulkCreateResult> {
  const result = await RuntimeREST.instance.post(
    appendConnectionId(
      `/api/runtime/object-model/instances/${schemaId}/bulk`,
      connectionId
    ),
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
  properties: Record<string, unknown>,
  connectionId?: string | null
): Promise<number> {
  const result = await RuntimeREST.instance.patch(
    appendConnectionId(
      `/api/runtime/object-model/instances/${schemaId}/bulk`,
      connectionId
    ),
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
  data: CsvExportRequest,
  connectionId?: string | null
): Promise<Blob> {
  const result = await RuntimeREST.instance.post(
    appendConnectionId(
      `/api/runtime/object-model/instances/schema/${schemaName}/export-csv`,
      connectionId
    ),
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
  data: CsvPreviewJsonRequest,
  connectionId?: string | null
): Promise<ImportPreviewResponse> {
  const result = await RuntimeREST.instance.post(
    appendConnectionId(
      `/api/runtime/object-model/instances/schema/${schemaName}/import-csv/preview`,
      connectionId
    ),
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
  data: CsvImportJsonRequest,
  connectionId?: string | null
): Promise<CsvImportResponse> {
  const result = await RuntimeREST.instance.post(
    appendConnectionId(
      `/api/runtime/object-model/instances/schema/${schemaName}/import-csv`,
      connectionId
    ),
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
