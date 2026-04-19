/* eslint-disable react-hooks/rules-of-hooks */
import { test as base, Page, Route } from '@playwright/test';
import type {
  ApiKey,
  ConnectionDto,
  ConnectionTypeDto,
  Instance,
  InvocationTrigger,
  WorkflowDto,
  Schema,
} from '@/generated/RuntaraRuntimeApi';
import { paginated } from './builders';

type JsonBody = Record<string, unknown> | Array<unknown>;

/**
 * Match `/api/runtime/[optional orgId]/<suffix>` — org_id is inserted by an axios
 * interceptor (see src/shared/queries/index.ts:35-41) so tests must tolerate both.
 * Only the query string is allowed as a trailing slot; subpaths do NOT match, so
 * `runtimeUrl('connections')` does not swallow `connections/types` or
 * `workflows` does not swallow `workflows/folders`.
 */
function runtimeUrl(suffix: string): RegExp {
  const escaped = suffix.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return new RegExp(`/api/runtime(?:/[^/]+)?/${escaped}(?:\\?[^/]*)?$`);
}

async function fulfill(
  route: Route,
  body: JsonBody | string,
  status = 200
): Promise<void> {
  await route.fulfill({
    status,
    contentType: 'application/json',
    body: typeof body === 'string' ? body : JSON.stringify(body),
  });
}

/**
 * High-level typed mock factory. Each call installs a `page.route()` handler for
 * the corresponding backend endpoint and responds with the provided fixture.
 * Returns nothing — the handler is active for the lifetime of the page.
 */
export interface MockApi {
  connections: {
    list: (
      page: Page,
      items: ConnectionDto[],
      opts?: { total?: number }
    ) => Promise<void>;
    get: (page: Page, id: string, connection: ConnectionDto) => Promise<void>;
    types: (page: Page, types: ConnectionTypeDto[]) => Promise<void>;
    typeById: (
      page: Page,
      integrationId: string,
      type: ConnectionTypeDto
    ) => Promise<void>;
    create: (page: Page, created: ConnectionDto) => Promise<void>;
    delete: (page: Page) => Promise<void>;
  };
  workflows: {
    list: (
      page: Page,
      items: WorkflowDto[],
      opts?: { total?: number }
    ) => Promise<void>;
    get: (page: Page, id: string, workflow: WorkflowDto) => Promise<void>;
    history: (
      page: Page,
      workflowId: string,
      instances: unknown[]
    ) => Promise<void>;
    instance: (
      page: Page,
      workflowId: string,
      instanceId: string,
      body: unknown
    ) => Promise<void>;
    logs: (
      page: Page,
      workflowId: string,
      instanceId: string,
      body: unknown
    ) => Promise<void>;
  };
  triggers: {
    list: (page: Page, triggers: InvocationTrigger[]) => Promise<void>;
    get: (page: Page, id: string, trigger: InvocationTrigger) => Promise<void>;
    create: (page: Page, created: InvocationTrigger) => Promise<void>;
  };
  objects: {
    schemas: {
      list: (page: Page, schemas: Schema[]) => Promise<void>;
      get: (page: Page, id: string, schema: Schema) => Promise<void>;
      create: (page: Page, schemaId: string) => Promise<void>;
      update: (page: Page) => Promise<void>;
    };
    instances: {
      listBySchemaId: (
        page: Page,
        schemaId: string,
        instances: Instance[],
        opts?: { total?: number }
      ) => Promise<void>;
      filterBySchemaName: (
        page: Page,
        schemaName: string,
        instances: Instance[],
        opts?: { total?: number }
      ) => Promise<void>;
      get: (
        page: Page,
        schemaId: string,
        instanceId: string,
        instance: Instance
      ) => Promise<void>;
      create: (page: Page, instanceId: string) => Promise<void>;
    };
  };
  analytics: {
    tenantMetrics: (page: Page, body: unknown) => Promise<void>;
    system: (page: Page, body: unknown) => Promise<void>;
    rateLimits: (page: Page, body: unknown) => Promise<void>;
  };
  files: {
    buckets: (page: Page, buckets: unknown[]) => Promise<void>;
    list: (page: Page, bucket: string, items: unknown[]) => Promise<void>;
  };
  apiKeys: {
    list: (page: Page, keys: ApiKey[]) => Promise<void>;
  };
  runtime: {
    health: (page: Page, ok?: boolean) => Promise<void>;
    metadata: (page: Page, body?: unknown) => Promise<void>;
  };
  invocationHistory: {
    list: (
      page: Page,
      entries: unknown[],
      opts?: { total?: number }
    ) => Promise<void>;
  };
  /** Catch-all: respond to any un-mocked /api/runtime call with 200 + empty body. Avoids noisy real-network hits in snapshot diffs. */
  fallthrough: (page: Page) => Promise<void>;
  /** Stubs every endpoint the sidebar/layout queries so any page renders without errors. */
  bootstrap: (page: Page) => Promise<void>;
  /** Raw escape hatch when the high-level helpers don't fit. */
  raw: (
    page: Page,
    url: string | RegExp,
    body: JsonBody | ((route: Route) => Promise<void>),
    opts?: { status?: number }
  ) => Promise<void>;
}

const factory: MockApi = {
  connections: {
    list: (page, items, opts) =>
      page.route(runtimeUrl('connections'), (route) =>
        fulfill(route, {
          connections: items,
          count: opts?.total ?? items.length,
          success: true,
        })
      ),
    get: (page, id, connection) =>
      page.route(runtimeUrl(`connections/${id}`), (route) =>
        fulfill(route, { connection, success: true })
      ),
    types: (page, types) =>
      page.route(runtimeUrl('connections/types'), (route) =>
        fulfill(route, { connectionTypes: types })
      ),
    typeById: (page, integrationId, type) =>
      page.route(runtimeUrl(`connections/types/${integrationId}`), (route) =>
        fulfill(route, { connectionType: type, success: true })
      ),
    create: (page, created) =>
      page.route(runtimeUrl('connections'), (route) => {
        if (route.request().method() === 'POST') {
          return fulfill(route, { connection: created, success: true }, 201);
        }
        return route.fallback();
      }),
    delete: (page) =>
      page.route(runtimeUrl('connections/.+'), (route) => {
        if (route.request().method() === 'DELETE') {
          return fulfill(route, { success: true });
        }
        return route.fallback();
      }),
  },
  workflows: {
    list: (page, items, opts) =>
      page.route(runtimeUrl('workflows'), (route) =>
        fulfill(route, {
          data: {
            content: items,
            number: 0,
            size: 20,
            totalElements: opts?.total ?? items.length,
            totalPages: 1,
            first: true,
            last: true,
          },
          success: true,
        })
      ),
    get: (page, id, workflow) =>
      page.route(runtimeUrl(`workflows/${id}`), (route) =>
        fulfill(route, { data: workflow, success: true })
      ),
    history: (page, workflowId, instances) =>
      page.route(runtimeUrl(`workflows/${workflowId}/history`), (route) =>
        fulfill(route, paginated(instances))
      ),
    instance: (page, workflowId, instanceId, body) =>
      page.route(
        runtimeUrl(`workflows/${workflowId}/history/${instanceId}`),
        (route) => fulfill(route, body as JsonBody)
      ),
    logs: (page, workflowId, instanceId, body) =>
      page.route(
        runtimeUrl(`workflows/${workflowId}/history/${instanceId}/logs`),
        (route) => fulfill(route, body as JsonBody)
      ),
  },
  triggers: {
    list: (page, triggers) =>
      page.route(runtimeUrl('triggers'), (route) =>
        fulfill(route, { data: triggers, success: true })
      ),
    get: (page, id, trigger) =>
      page.route(runtimeUrl(`triggers/${id}`), (route) =>
        fulfill(route, { data: trigger, success: true })
      ),
    create: (page, created) =>
      page.route(runtimeUrl('triggers'), (route) => {
        if (route.request().method() === 'POST') {
          return fulfill(route, { data: created, success: true }, 201);
        }
        return route.fallback();
      }),
  },
  objects: {
    schemas: {
      list: (page, schemas) =>
        page.route(runtimeUrl('object-model/schemas'), (route) =>
          fulfill(route, { schemas, totalCount: schemas.length })
        ),
      get: (page, id, schema) =>
        page.route(runtimeUrl(`object-model/schemas/${id}`), (route) =>
          fulfill(route, { schema, success: true })
        ),
      create: (page, schemaId) =>
        page.route(runtimeUrl('object-model/schemas'), (route) => {
          if (route.request().method() === 'POST') {
            return fulfill(route, { schemaId, success: true }, 201);
          }
          return route.fallback();
        }),
      update: (page) =>
        page.route(runtimeUrl('object-model/schemas/.+'), (route) => {
          if (route.request().method() === 'PUT') {
            return fulfill(route, { success: true });
          }
          return route.fallback();
        }),
    },
    instances: {
      listBySchemaId: (page, schemaId, instances, opts) =>
        page.route(
          runtimeUrl(`object-model/instances/schema/${schemaId}`),
          (route) =>
            fulfill(route, {
              instances,
              totalCount: opts?.total ?? instances.length,
            })
        ),
      filterBySchemaName: (page, schemaName, instances, opts) =>
        page.route(
          runtimeUrl(`object-model/instances/schema/name/${schemaName}`),
          (route) =>
            fulfill(route, {
              instances,
              totalCount: opts?.total ?? instances.length,
            })
        ),
      get: (page, schemaId, instanceId, instance) =>
        page.route(
          runtimeUrl(`object-model/instances/${schemaId}/${instanceId}`),
          (route) => fulfill(route, { instance, success: true })
        ),
      create: (page, instanceId) =>
        page.route(runtimeUrl('object-model/instances'), (route) => {
          if (route.request().method() === 'POST') {
            return fulfill(route, { instanceId, success: true }, 201);
          }
          return route.fallback();
        }),
    },
  },
  analytics: {
    tenantMetrics: (page, body) =>
      page.route(runtimeUrl('metrics/tenant'), (route) =>
        fulfill(route, body as JsonBody)
      ),
    system: (page, body) =>
      page.route(runtimeUrl('analytics/system'), (route) =>
        fulfill(route, body as JsonBody)
      ),
    rateLimits: (page, body) =>
      page.route(runtimeUrl('connections/.+/rate-limit-status'), (route) =>
        fulfill(route, body as JsonBody)
      ),
  },
  files: {
    buckets: (page, buckets) =>
      page.route(runtimeUrl('files/buckets'), (route) =>
        fulfill(route, { buckets })
      ),
    list: (page, bucket, items) =>
      page.route(runtimeUrl(`files/${bucket}`), (route) =>
        fulfill(route, { items, totalCount: items.length })
      ),
  },
  apiKeys: {
    list: (page, keys) =>
      page.route(runtimeUrl('api-keys'), (route) => fulfill(route, keys)),
  },
  runtime: {
    health: (page, ok = true) =>
      page.route(/\/api\/(runtime|gateway)\/?(health|healthz)?/, (route) =>
        fulfill(
          route,
          ok
            ? { status: 'ok', database: 'up' }
            : { status: 'degraded', database: 'up' }
        )
      ),
    metadata: (page, body = { stepTypes: [] }) =>
      page.route(runtimeUrl('metadata/workflow/step-types'), (route) =>
        fulfill(route, body as JsonBody)
      ),
  },
  invocationHistory: {
    list: (page, entries, opts) =>
      page.route(runtimeUrl('executions'), (route) =>
        fulfill(route, {
          data: {
            content: entries,
            number: 0,
            size: 10,
            numberOfElements: entries.length,
            totalElements: opts?.total ?? entries.length,
            totalPages: 1,
            first: true,
            last: true,
          },
          success: true,
        })
      ),
  },
  fallthrough: (page) =>
    page.route(/\/api\/runtime\//, (route) => fulfill(route, {})),
  bootstrap: async (page) => {
    // Register the catch-all FIRST so later handlers (specific endpoints + spec-level
    // overrides) take precedence (Playwright matches page.route handlers LIFO).
    await page.route(/\/api\/runtime\//, (route) => fulfill(route, {}));
    await page.route(/\/api\/management\//, (route) => fulfill(route, {}));
    // Sidebar → useFolders; Sidebar → menu
    await page.route(runtimeUrl('workflows/folders'), (route) =>
      fulfill(route, { folders: [] })
    );
    // Health check hits ManagementAPI /health (no /api/runtime prefix)
    await page.route(/\/health(\?.*)?$/, (route) =>
      fulfill(route, { status: 'ok' })
    );
    // User groups hook reads profile; some layouts may poll tenant info
    await page.route(runtimeUrl('metrics/tenant'), (route) =>
      fulfill(route, { workflowsCount: 0, connectionsCount: 0 })
    );
    // Connection categories/types are fetched by several pages
    await page.route(runtimeUrl('connections/categories'), (route) =>
      fulfill(route, { categories: [] })
    );
    await page.route(runtimeUrl('connections/auth-types'), (route) =>
      fulfill(route, { authTypes: [] })
    );
    await page.route(runtimeUrl('connections/types'), (route) =>
      fulfill(route, { connectionTypes: [] })
    );
  },
  raw: (page, url, body, opts) =>
    page.route(url, (route) => {
      if (typeof body === 'function') {
        return body(route);
      }
      return fulfill(route, body, opts?.status);
    }),
};

export interface MockFixtures {
  mockApi: MockApi;
}

export const test = base.extend<MockFixtures>({
  mockApi: async ({}, use) => {
    await use(factory);
  },
});

export { expect } from '@playwright/test';
