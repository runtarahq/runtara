import type {
  ApiKey,
  ConnectionDto,
  ConnectionTypeDto,
  Instance,
  InvocationTrigger,
  WorkflowDto,
  Schema,
} from '@/generated/RuntaraRuntimeApi';

type DeepPartial<T> = T extends object
  ? { [K in keyof T]?: DeepPartial<T[K]> }
  : T;

let idCounter = 0;
function nextId(prefix: string): string {
  idCounter += 1;
  return `${prefix}_${String(idCounter).padStart(6, '0')}`;
}

const nowIso = () => new Date('2026-01-01T12:00:00Z').toISOString();

export function buildConnection(
  overrides: DeepPartial<ConnectionDto> = {}
): ConnectionDto {
  return {
    id: nextId('conn'),
    title: 'Test Connection',
    integrationId: 'http',
    status: 'ACTIVE' as ConnectionDto['status'],
    isDefaultFileStorage: false,
    tenantId: 'tenant_e2e',
    createdAt: nowIso(),
    updatedAt: nowIso(),
    ...overrides,
  } as ConnectionDto;
}

export function buildConnectionType(
  overrides: DeepPartial<ConnectionTypeDto> = {}
): ConnectionTypeDto {
  return {
    integrationId: 'http',
    displayName: 'HTTP',
    description: 'Generic HTTP connection',
    category: 'api',
    fields: [],
    ...overrides,
  } as ConnectionTypeDto;
}

export function buildWorkflow(
  overrides: DeepPartial<WorkflowDto> = {}
): WorkflowDto {
  return {
    id: nextId('scn'),
    name: 'Test Workflow',
    description: 'Mock workflow',
    currentVersionNumber: 1,
    lastVersionNumber: 1,
    executionGraph: { steps: [] },
    inputSchema: {},
    outputSchema: {},
    created: nowIso(),
    updated: nowIso(),
    path: '/',
    ...overrides,
  } as WorkflowDto;
}

export function buildTrigger(
  overrides: DeepPartial<InvocationTrigger> = {}
): InvocationTrigger {
  return {
    id: nextId('trg'),
    active: true,
    configuration: {},
    created_at: nowIso(),
    workflow_id: 'scn_000001',
    ...overrides,
  } as unknown as InvocationTrigger;
}

export function buildSchema(overrides: DeepPartial<Schema> = {}): Schema {
  return {
    id: nextId('sch'),
    name: 'TestSchema',
    tableName: 'test_schema',
    tenantId: 'tenant_e2e',
    columns: [
      {
        name: 'id',
        type: 'STRING' as any,
      } as any,
    ],
    createdAt: nowIso(),
    updatedAt: nowIso(),
    ...overrides,
  } as Schema;
}

export function buildInstance(
  schemaId: string,
  overrides: DeepPartial<Instance> = {}
): Instance {
  return {
    id: nextId('inst'),
    schemaId,
    schemaName: 'TestSchema',
    tenantId: 'tenant_e2e',
    properties: {},
    createdAt: nowIso(),
    updatedAt: nowIso(),
    ...overrides,
  } as Instance;
}

export function buildApiKey(overrides: DeepPartial<ApiKey> = {}): ApiKey {
  return {
    id: nextId('key'),
    name: 'Test API Key',
    key_prefix: 'smo_test',
    org_id: 'org_mocked_e2e',
    is_revoked: false,
    created_at: nowIso(),
    last_used_at: null,
    ...overrides,
  } as ApiKey;
}

export const paginated = <T>(
  items: T[],
  page = 0,
  size = 20,
  total?: number
) => ({
  content: items,
  number: page,
  size,
  totalElements: total ?? items.length,
  totalPages: Math.max(1, Math.ceil((total ?? items.length) / size)),
});

export const listResponse = <T>(items: T[], key: string, total?: number) =>
  ({
    [key]: items,
    total: total ?? items.length,
    totalCount: total ?? items.length,
  }) as Record<string, unknown>;
