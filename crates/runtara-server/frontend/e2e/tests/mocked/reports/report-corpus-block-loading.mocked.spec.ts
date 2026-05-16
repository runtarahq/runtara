// Phase 0 of the reports refactor: assert each report block type loads in
// the viewer without crashing. This is the FE half of the safety net — the
// backend half lives in `crates/runtara-server/tests/reports_corpus.rs`,
// `reports_runtime_corpus.rs`, and `reports_render_corpus.rs`.
//
// See `docs/reports-refactoring-plan.md` Phase 0.
import type { Page, Route } from '@playwright/test';
import {
  buildObjectModelConnection,
  expect,
  test,
  type MockApi,
} from '../../../fixtures';
import { appPath } from '../../../utils/app-path';
import type { Schema } from '../../../../src/generated/RuntaraRuntimeApi';
import type {
  ReportBlockDefinition,
  ReportBlockResult,
  ReportDefinition,
  ReportDto,
  ReportRenderResponse,
} from '../../../../src/features/reports/types';

const TENANT = 'tenant_corpus_loading';

function runtimeUrl(suffix: string): RegExp {
  const escaped = suffix.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return new RegExp(`/api/runtime(?:/[^/]+)?/${escaped}(?:\\?[^/]*)?$`);
}

async function fulfill(route: Route, body: unknown, status = 200) {
  await route.fulfill({
    status,
    contentType: 'application/json',
    body: JSON.stringify(body),
  });
}

function schema(name: string, fields: string[]): Schema {
  return {
    id: name,
    name,
    tableName: name,
    tenantId: TENANT,
    createdAt: '2026-05-15T00:00:00Z',
    updatedAt: '2026-05-15T00:00:00Z',
    columns: fields.map((field) => ({ name: field, type: 'string' })),
  } as Schema;
}

const SCHEMAS: Schema[] = [
  schema('Order', [
    'id',
    'order_id',
    'customer_id',
    'customer_name',
    'customer_email',
    'status',
    'total_amount',
    'created_at',
  ]),
  schema('Customer', [
    'id',
    'name',
    'email',
    'status',
    'created_at',
    'billing_address',
    'recent_orders',
  ]),
];

function baseDefinition(
  blocks: ReportBlockDefinition[],
  extras: Partial<ReportDefinition> = {}
): ReportDefinition {
  return {
    definitionVersion: 1,
    filters: [],
    blocks,
    ...extras,
  };
}

function reportFor(id: string, definition: ReportDefinition): ReportDto {
  return {
    id,
    slug: id,
    name: `corpus ${id}`,
    description: null,
    tags: [],
    status: 'published',
    definitionVersion: definition.definitionVersion,
    definition,
    createdAt: '2026-05-15T00:00:00Z',
    updatedAt: '2026-05-15T00:00:00Z',
  };
}

function renderResponse(
  reportId: string,
  blocks: Record<string, ReportBlockResult>
): ReportRenderResponse {
  return {
    success: true,
    report: { id: reportId, definitionVersion: 1 },
    resolvedFilters: {},
    blocks,
    errors: [],
  };
}

async function gotoAppRoute(page: Page, path: string) {
  await page.goto(appPath('/'));
  await page.evaluate((routePath) => {
    const basePath = new URL(document.baseURI).pathname.replace(/\/$/, '');
    const normalizedPath = routePath.startsWith('/')
      ? routePath
      : `/${routePath}`;
    window.history.pushState({}, '', `${basePath}${normalizedPath}`);
    window.dispatchEvent(new PopStateEvent('popstate'));
  }, path);
}

interface OpenViewerOpts {
  reportId: string;
  definition: ReportDefinition;
  blocks: Record<string, ReportBlockResult>;
}

async function openReportViewer(
  { page, mockApi }: { page: Page; mockApi: MockApi },
  { reportId, definition, blocks }: OpenViewerOpts
) {
  await mockApi.bootstrap(page);
  await mockApi.connections.list(page, [
    buildObjectModelConnection({ id: 'conn_object_model_default' }),
  ]);
  await mockApi.objects.schemas.list(page, SCHEMAS);
  await mockApi.raw(page, runtimeUrl(`reports/${reportId}`), {
    report: reportFor(reportId, definition),
  });
  await mockApi.raw(
    page,
    runtimeUrl(`reports/${reportId}/render`),
    async (route) => {
      await fulfill(route, renderResponse(reportId, blocks));
    }
  );
  await gotoAppRoute(page, `/reports/${reportId}`);
}

test.describe('reports corpus — viewer block-type loading (mocked)', () => {
  test('markdown block renders its content', async ({ page, mockApi }) => {
    const reportId = 'corpus_markdown';
    const definition = baseDefinition([
      {
        id: 'intro',
        type: 'markdown',
        source: { schema: '', mode: 'filter' },
        markdown: {
          content: '# Hello corpus\n\nMarkdown block under test.',
        },
      },
    ]);
    const blocks: Record<string, ReportBlockResult> = {
      intro: {
        type: 'markdown',
        status: 'ready',
        data: { content: '# Hello corpus\n\nMarkdown block under test.' },
      },
    };

    await openReportViewer({ page, mockApi }, { reportId, definition, blocks });

    await expect(
      page.getByRole('heading', { name: 'Hello corpus' })
    ).toBeVisible();
    await expect(page.getByText('Markdown block under test.')).toBeVisible();
  });

  test('table block renders columns and rows', async ({ page, mockApi }) => {
    const reportId = 'corpus_table';
    const definition = baseDefinition([
      {
        id: 'orders_table',
        type: 'table',
        title: 'Recent orders',
        source: { schema: 'Order', mode: 'filter' },
        table: {
          columns: [
            { field: 'order_id', label: 'Order' },
            { field: 'status', label: 'Status' },
            { field: 'total_amount', label: 'Total', format: 'currency' },
          ],
        },
      },
    ]);
    const blocks: Record<string, ReportBlockResult> = {
      orders_table: {
        type: 'table',
        status: 'ready',
        title: 'Recent orders',
        data: {
          columns: [
            { key: 'order_id', label: 'Order', type: 'string' },
            { key: 'status', label: 'Status', type: 'string' },
            { key: 'total_amount', label: 'Total', type: 'number' },
          ],
          rows: [
            { order_id: 'ORD-001', status: 'active', total_amount: 199 },
            { order_id: 'ORD-002', status: 'paused', total_amount: 49.5 },
          ],
          page: {
            offset: 0,
            size: 50,
            totalCount: 2,
            hasNextPage: false,
          },
        },
      },
    };

    await openReportViewer({ page, mockApi }, { reportId, definition, blocks });

    await expect(
      page.getByRole('heading', { name: 'Recent orders' })
    ).toBeVisible();
    await expect(page.getByText('ORD-001')).toBeVisible();
    await expect(page.getByText('ORD-002')).toBeVisible();
  });

  test('chart block renders title and recharts container', async ({
    page,
    mockApi,
  }) => {
    const reportId = 'corpus_chart';
    const definition = baseDefinition([
      {
        id: 'revenue_chart',
        type: 'chart',
        title: 'Revenue by day',
        source: {
          schema: 'Order',
          mode: 'aggregate',
          groupBy: ['created_at'],
          aggregates: [{ alias: 'revenue', op: 'sum', field: 'total_amount' }],
        },
        chart: {
          kind: 'line',
          x: 'created_at',
          series: [{ field: 'revenue', label: 'Revenue' }],
        },
      },
    ]);
    const blocks: Record<string, ReportBlockResult> = {
      revenue_chart: {
        type: 'chart',
        status: 'ready',
        title: 'Revenue by day',
        data: {
          columns: [
            { key: 'created_at', label: 'Date', type: 'string' },
            { key: 'revenue', label: 'Revenue', type: 'number' },
          ],
          rows: [
            { created_at: '2026-01-01', revenue: 12000 },
            { created_at: '2026-01-02', revenue: 14000 },
          ],
        },
      },
    };

    await openReportViewer({ page, mockApi }, { reportId, definition, blocks });

    await expect(
      page.getByRole('heading', { name: 'Revenue by day' })
    ).toBeVisible();
    // Recharts renders an SVG with a class containing "recharts".
    await expect(page.locator('svg.recharts-surface').first()).toBeVisible();
  });

  test('metric block renders its formatted value', async ({
    page,
    mockApi,
  }) => {
    const reportId = 'corpus_metric';
    const definition = baseDefinition([
      {
        id: 'total_revenue',
        type: 'metric',
        title: 'Total revenue',
        source: {
          schema: 'Order',
          mode: 'aggregate',
          aggregates: [{ alias: 'value', op: 'sum', field: 'total_amount' }],
        },
        metric: { valueField: 'value', label: 'Revenue', format: 'currency' },
      },
    ]);
    const blocks: Record<string, ReportBlockResult> = {
      total_revenue: {
        type: 'metric',
        status: 'ready',
        title: 'Total revenue',
        data: { value: 199500 },
      },
    };

    await openReportViewer({ page, mockApi }, { reportId, definition, blocks });

    await expect(
      page.getByRole('heading', { name: 'Total revenue' })
    ).toBeVisible();
    // currency format renders with the locale currency symbol.
    await expect(
      page.getByText(/\$\s?199,?500|199,500/).first()
    ).toBeVisible();
  });

  test('card block renders its group fields', async ({ page, mockApi }) => {
    const reportId = 'corpus_card';
    const definition = baseDefinition([
      {
        id: 'customer_card',
        type: 'card',
        title: 'Customer details',
        source: { schema: 'Customer', mode: 'filter', limit: 1 },
        card: {
          groups: [
            {
              id: 'g_identity',
              title: 'Identity',
              columns: 2,
              fields: [
                { field: 'name', label: 'Name', kind: 'value' },
                { field: 'email', label: 'Email', kind: 'value' },
              ],
            },
          ],
        },
      },
    ]);
    const blocks: Record<string, ReportBlockResult> = {
      customer_card: {
        type: 'card',
        status: 'ready',
        title: 'Customer details',
        data: {
          row: {
            name: 'Acme Corp',
            email: 'ops@acme.example',
          },
        },
      },
    };

    await openReportViewer({ page, mockApi }, { reportId, definition, blocks });

    await expect(
      page.getByRole('heading', { name: 'Customer details' })
    ).toBeVisible();
    await expect(page.getByText('Acme Corp')).toBeVisible();
    await expect(page.getByText('ops@acme.example')).toBeVisible();
  });

  test('block with status=error surfaces its error message', async ({
    page,
    mockApi,
  }) => {
    // The fixtures we ship today render through the BLOCK_RENDER_FAILED path
    // when their schemas aren't seeded — see `reports_render_corpus.rs`. This
    // FE test asserts that path is surfaced in the viewer, not swallowed.
    const reportId = 'corpus_block_error';
    const definition = baseDefinition([
      {
        id: 'broken_table',
        type: 'table',
        title: 'Recent orders',
        source: { schema: 'NotARealSchema', mode: 'filter' },
        table: { columns: [{ field: 'id', label: 'ID' }] },
      },
    ]);
    const blocks: Record<string, ReportBlockResult> = {
      broken_table: {
        type: 'table',
        status: 'error',
        title: 'Recent orders',
        error: {
          code: 'BLOCK_RENDER_FAILED',
          message: 'Schema not found: NotARealSchema',
          blockId: 'broken_table',
        },
      },
    };

    await openReportViewer({ page, mockApi }, { reportId, definition, blocks });

    await expect(
      page.getByRole('heading', { name: 'Recent orders' })
    ).toBeVisible();
    await expect(page.getByText(/Schema not found/)).toBeVisible();
  });
});
