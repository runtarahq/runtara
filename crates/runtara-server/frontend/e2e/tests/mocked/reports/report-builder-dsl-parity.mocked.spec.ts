import type { Page, Route } from '@playwright/test';
import { expect, test, type MockApi } from '../../../fixtures';
import { appPath } from '../../../utils/app-path';
import type { Schema } from '../../../../src/generated/RuntaraRuntimeApi';
import type {
  ReportBlockDefinition,
  ReportBlockResult,
  ReportDefinition,
  ReportDto,
  ReportRenderResponse,
  UpdateReportRequest,
} from '../../../../src/features/reports/types';

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
    tenantId: 'tenant_syn_410',
    createdAt: '2026-05-15T00:00:00Z',
    updatedAt: '2026-05-15T00:00:00Z',
    columns: fields.map((field) => ({ name: field, type: 'string' })),
  } as Schema;
}

const SCHEMAS = [
  schema('orders', [
    'id',
    'status',
    'amount',
    'customer_id',
    'customer_name',
    'customer_email',
    'customer_url',
    'country_id',
    'created_at',
    'notes',
    'priority',
    'line_items',
    'active',
    'process',
    'open',
    'run',
  ]),
  schema('customers', ['id', 'name', 'country_id', 'email', 'active']),
  schema('countries', ['id', 'name']),
];

function reportFor(id: string, definition: ReportDefinition): ReportDto {
  return {
    id,
    slug: id,
    name: `SYN-410 ${id}`,
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

async function openReportEditor({
  page,
  mockApi,
  id,
  definition,
  previewBlocks = {},
}: {
  page: Page;
  mockApi: MockApi;
  id: string;
  definition: ReportDefinition;
  previewBlocks?: Record<string, ReportBlockResult>;
}) {
  await mockApi.bootstrap(page);
  await mockApi.objects.schemas.list(page, SCHEMAS);

  const report = reportFor(id, definition);
  let saved: UpdateReportRequest | null = null;

  await mockApi.raw(page, runtimeUrl(`reports/${id}`), async (route) => {
    const method = route.request().method();
    if (method === 'GET') {
      await fulfill(route, { report });
      return;
    }
    if (method === 'PUT') {
      saved = route.request().postDataJSON() as UpdateReportRequest;
      await fulfill(route, {
        report: {
          ...report,
          ...saved,
          definitionVersion: saved.definition.definitionVersion,
          updatedAt: '2026-05-15T00:01:00Z',
        },
      });
      return;
    }
    await route.fallback();
  });
  await mockApi.raw(
    page,
    runtimeUrl('reports/preview'),
    renderResponse(id, previewBlocks)
  );
  await mockApi.raw(page, runtimeUrl('reports/validate'), {
    valid: true,
    errors: [],
    warnings: [],
  });

  await gotoAppRoute(page, `/reports/${id}?edit=1`);
  await expect(page.getByRole('button', { name: /^Save$/ })).toBeVisible();
  await expect(
    page.getByText('This report uses advanced features')
  ).toHaveCount(0);

  return {
    getSaved: () => saved,
  };
}

async function saveThroughWizard(
  page: Page,
  getSaved: () => UpdateReportRequest | null
): Promise<ReportDefinition> {
  const firstBlockTitle = page
    .locator('input[placeholder="Untitled block"]')
    .first();
  await expect(firstBlockTitle).toBeVisible();
  const currentTitle = await firstBlockTitle.inputValue();
  await firstBlockTitle.fill(`${currentTitle || 'Block'} parity`);
  await page.getByRole('button', { name: /^Save$/ }).click();
  await expect.poll(() => getSaved() !== null).toBe(true);
  return getSaved()!.definition;
}

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

function tableBlock(
  id = 'orders',
  patch: Partial<ReportBlockDefinition> = {}
): ReportBlockDefinition {
  return {
    id,
    type: 'table',
    title: 'Orders',
    source: { schema: 'orders', mode: 'filter' },
    table: {
      columns: [
        { field: 'id', label: 'ID' },
        { field: 'status', label: 'Status' },
      ],
    },
    ...patch,
  };
}

function block(
  definition: ReportDefinition,
  id: string
): ReportBlockDefinition {
  const match = definition.blocks.find((candidate) => candidate.id === id);
  expect(match, `block ${id}`).toBeTruthy();
  return match!;
}

test.describe('SYN-410 report builder DSL parity (mocked)', () => {
  test('01 saves table workflow-button columns and table actions', async ({
    page,
    mockApi,
  }) => {
    const definition = baseDefinition([
      tableBlock('orders', {
        table: {
          columns: [
            { field: 'id', label: 'ID' },
            {
              field: 'process',
              label: 'Process',
              type: 'workflow_button',
              workflowAction: {
                workflowId: 'workflow_process',
                label: 'Process',
                reloadBlock: true,
                context: { mode: 'row' },
              },
            },
            {
              field: 'open',
              label: 'Open',
              type: 'interaction_buttons',
              interactionButtons: [
                {
                  id: 'open_detail',
                  label: 'Open',
                  icon: 'arrow_right',
                  actions: [
                    {
                      type: 'set_filter',
                      filterId: 'order_id',
                      valueFrom: 'datum.id',
                    },
                  ],
                },
              ],
            },
          ],
          selectable: true,
          actions: [
            {
              id: 'bulk_archive',
              label: 'Archive selected',
              workflowAction: {
                workflowId: 'workflow_archive',
                label: 'Archive',
                reloadBlock: true,
                context: { mode: 'selection' },
              },
            },
          ],
        },
      }),
    ]);

    const { getSaved } = await openReportEditor({
      page,
      mockApi,
      id: 'table_actions',
      definition,
    });
    const saved = await saveThroughWizard(page, getSaved);
    const table = block(saved, 'orders').table!;

    expect(table.selectable).toBe(true);
    expect(table.actions?.[0]).toMatchObject({
      id: 'bulk_archive',
      workflowAction: { workflowId: 'workflow_archive' },
    });
    expect(table.columns?.map((column) => column.type ?? 'value')).toEqual([
      'value',
      'workflow_button',
      'interaction_buttons',
    ]);
    expect(
      table.columns?.find((column) => column.type === 'workflow_button')
        ?.workflowAction
    ).toMatchObject({ workflowId: 'workflow_process' });
  });

  test('02 saves configurable table pagination and default sort', async ({
    page,
    mockApi,
  }) => {
    const definition = baseDefinition([
      tableBlock('orders', {
        table: {
          columns: [
            { field: 'status', label: 'Status' },
            { field: 'amount', label: 'Amount', format: 'currency' },
          ],
          defaultSort: [{ field: 'amount', direction: 'desc' }],
          pagination: {
            defaultPageSize: 25,
            allowedPageSizes: [10, 25, 100],
          },
        },
      }),
    ]);

    const { getSaved } = await openReportEditor({
      page,
      mockApi,
      id: 'table_pagination_sort',
      definition,
    });
    const saved = await saveThroughWizard(page, getSaved);
    const table = block(saved, 'orders').table!;

    expect(table.defaultSort).toEqual([{ field: 'amount', direction: 'desc' }]);
    expect(table.pagination).toEqual({
      defaultPageSize: 25,
      allowedPageSizes: [10, 25, 100],
    });
  });

  test('03 saves block visibility, hideWhenEmpty, and lazy loading', async ({
    page,
    mockApi,
  }) => {
    const definition = baseDefinition(
      [
        tableBlock('orders', {
          lazy: true,
          hideWhenEmpty: true,
          showWhen: { filter: 'status_filter', equals: 'open' },
        }),
      ],
      {
        filters: [
          {
            id: 'status_filter',
            label: 'Status',
            type: 'select',
            appliesTo: [{ field: 'status', op: 'eq' }],
          },
        ],
      }
    );

    const { getSaved } = await openReportEditor({
      page,
      mockApi,
      id: 'visibility_lazy',
      definition,
    });
    const saved = await saveThroughWizard(page, getSaved);

    expect(block(saved, 'orders')).toMatchObject({
      lazy: true,
      hideWhenEmpty: true,
      showWhen: { filter: 'status_filter', equals: 'open' },
    });
  });

  test('04 saves row, cell, and point click interactions', async ({
    page,
    mockApi,
  }) => {
    const definition = baseDefinition(
      [
        tableBlock('orders', {
          interactions: [
            {
              id: 'open_order',
              trigger: { event: 'row_click' },
              actions: [
                {
                  type: 'set_filter',
                  filterId: 'order_id',
                  valueFrom: 'datum.id',
                },
                { type: 'navigate_view', viewId: 'detail' },
              ],
            },
            {
              id: 'filter_status',
              trigger: { event: 'cell_click', field: 'status' },
              actions: [
                {
                  type: 'set_filter',
                  filterId: 'status_filter',
                  valueFrom: 'datum.value',
                },
              ],
            },
          ],
        }),
        {
          id: 'status_chart',
          type: 'chart',
          title: 'Status',
          source: {
            schema: 'orders',
            mode: 'aggregate',
            groupBy: ['status'],
            aggregates: [{ alias: 'value', op: 'count' }],
          },
          chart: {
            kind: 'bar',
            x: 'status',
            series: [{ field: 'value' }],
          },
          interactions: [
            {
              id: 'chart_status',
              trigger: { event: 'point_click', field: 'status' },
              actions: [
                {
                  type: 'set_filter',
                  filterId: 'status_filter',
                  valueFrom: 'datum.status',
                },
              ],
            },
          ],
        },
      ],
      {
        filters: [
          {
            id: 'order_id',
            label: 'Order',
            type: 'select',
            appliesTo: [{ field: 'id', op: 'eq' }],
          },
          {
            id: 'status_filter',
            label: 'Status',
            type: 'select',
            appliesTo: [{ field: 'status', op: 'eq' }],
          },
        ],
      }
    );

    const { getSaved } = await openReportEditor({
      page,
      mockApi,
      id: 'block_interactions',
      definition,
    });
    const saved = await saveThroughWizard(page, getSaved);

    expect(
      block(saved, 'orders').interactions?.map((item) => item.trigger)
    ).toEqual([
      { event: 'row_click' },
      { event: 'cell_click', field: 'status' },
    ]);
    expect(block(saved, 'status_chart').interactions?.[0].trigger).toEqual({
      event: 'point_click',
      field: 'status',
    });
  });

  test('05 saves multi-view reports with drilldown breadcrumbs', async ({
    page,
    mockApi,
  }) => {
    const definition = baseDefinition(
      [tableBlock('orders', { table: { columns: [{ field: 'id' }] } })],
      {
        layout: [
          {
            id: 'main',
            type: 'grid',
            columns: 1,
            items: [{ blockId: 'orders' }],
          },
        ],
        views: [
          {
            id: 'list',
            title: 'Orders',
            layout: [
              {
                id: 'list_grid',
                type: 'grid',
                columns: 1,
                items: [{ blockId: 'orders' }],
              },
            ],
          },
          {
            id: 'detail',
            titleFrom: 'filters.order_id',
            parentViewId: 'list',
            clearFiltersOnBack: ['order_id'],
            breadcrumb: [
              {
                label: 'Orders',
                viewId: 'list',
                clearFilters: ['order_id'],
              },
            ],
            layout: [
              {
                id: 'detail_grid',
                type: 'grid',
                columns: 1,
                items: [{ blockId: 'orders' }],
              },
            ],
          },
        ],
      }
    );

    const { getSaved } = await openReportEditor({
      page,
      mockApi,
      id: 'views_breadcrumbs',
      definition,
    });
    const saved = await saveThroughWizard(page, getSaved);

    expect(saved.views).toEqual(definition.views);
  });

  test('06 saves block-level filters', async ({ page, mockApi }) => {
    const blockFilter = {
      id: 'status_filter',
      label: 'Status',
      type: 'select' as const,
      appliesTo: [{ blockId: 'orders', field: 'status', op: 'eq' }],
      options: {
        source: 'static' as const,
        values: [
          { label: 'Open', value: 'open' },
          { label: 'Closed', value: 'closed' },
        ],
      },
    };
    const definition = baseDefinition([
      tableBlock('orders', {
        filters: [blockFilter],
      }),
    ]);

    const { getSaved } = await openReportEditor({
      page,
      mockApi,
      id: 'block_filters',
      definition,
    });
    const saved = await saveThroughWizard(page, getSaved);

    expect(block(saved, 'orders').filters).toEqual([blockFilter]);
  });

  test('07 saves editable cell writeback editors', async ({
    page,
    mockApi,
  }) => {
    const definition = baseDefinition([
      tableBlock('orders', {
        table: {
          columns: [
            {
              field: 'status',
              label: 'Status',
              format: 'pill',
              editable: true,
              editor: {
                kind: 'select',
                options: [
                  { label: 'Open', value: 'open' },
                  { label: 'Closed', value: 'closed' },
                ],
              },
            },
          ],
        },
      }),
      {
        id: 'order_card',
        type: 'card',
        title: 'Order',
        source: { schema: 'orders', mode: 'filter' },
        card: {
          groups: [
            {
              id: 'main',
              fields: [
                {
                  field: 'amount',
                  label: 'Amount',
                  editable: true,
                  editor: { kind: 'number', min: 0, step: 0.01 },
                },
              ],
            },
          ],
        },
      },
    ]);

    const { getSaved } = await openReportEditor({
      page,
      mockApi,
      id: 'editable_cells',
      definition,
    });
    const saved = await saveThroughWizard(page, getSaved);

    expect(block(saved, 'orders').table?.columns?.[0]).toMatchObject({
      editable: true,
      editor: { kind: 'select' },
    });
    expect(block(saved, 'order_card').card?.groups[0].fields[0]).toMatchObject({
      editable: true,
      editor: { kind: 'number', min: 0, step: 0.01 },
    });
  });

  test('08 saves semantic datasets and per-block dataset queries', async ({
    page,
    mockApi,
  }) => {
    const definition = baseDefinition(
      [
        {
          id: 'totals_table',
          type: 'table',
          title: 'Totals',
          source: { schema: '' },
          dataset: {
            id: 'order_totals',
            dimensions: ['status'],
            measures: ['total_amount'],
            orderBy: [{ field: 'total_amount', direction: 'desc' }],
            limit: 50,
          },
        },
      ],
      {
        datasets: [
          {
            id: 'order_totals',
            label: 'Order totals',
            source: { schema: 'orders' },
            dimensions: [{ field: 'status', label: 'Status', type: 'string' }],
            measures: [
              {
                id: 'total_amount',
                label: 'Total',
                op: 'sum',
                field: 'amount',
                format: 'currency',
              },
              {
                id: 'count',
                label: 'Count',
                op: 'count',
                format: 'number',
              },
            ],
          },
        ],
      }
    );

    const { getSaved } = await openReportEditor({
      page,
      mockApi,
      id: 'datasets',
      definition,
    });
    const saved = await saveThroughWizard(page, getSaved);
    const totals = block(saved, 'totals_table');

    expect(saved.datasets?.[0]).toMatchObject({
      id: 'order_totals',
      source: { schema: 'orders' },
    });
    expect(totals.dataset).toMatchObject({
      id: 'order_totals',
      dimensions: ['status'],
      measures: ['total_amount'],
      limit: 50,
    });
    expect(totals.table?.columns?.map((column) => column.field)).toEqual([
      'status',
      'total_amount',
    ]);
  });

  test('09 saves workflow_runtime and system source kinds', async ({
    page,
    mockApi,
  }) => {
    const definition = baseDefinition([
      {
        id: 'runs',
        type: 'table',
        title: 'Workflow runs',
        source: {
          kind: 'workflow_runtime',
          schema: '',
          entity: 'instances',
          workflowId: 'inventory_sync',
          mode: 'filter',
        },
        table: {
          columns: [
            { field: 'instanceId', label: 'Instance' },
            { field: 'status', label: 'Status', format: 'pill' },
          ],
        },
      },
      {
        id: 'rate_limit_timeline',
        type: 'chart',
        title: 'Rate limit timeline',
        source: {
          kind: 'system',
          schema: '',
          entity: 'connection_rate_limit_timeline',
          mode: 'aggregate',
          granularity: 'hourly',
          interval: '24h',
          groupBy: ['bucketTime'],
          aggregates: [{ alias: 'value', op: 'count' }],
        },
        chart: {
          kind: 'bar',
          x: 'bucketTime',
          series: [{ field: 'value' }],
        },
      },
    ]);

    const { getSaved } = await openReportEditor({
      page,
      mockApi,
      id: 'source_kinds',
      definition,
    });
    const saved = await saveThroughWizard(page, getSaved);

    expect(block(saved, 'runs').source).toMatchObject({
      kind: 'workflow_runtime',
      schema: '',
      entity: 'instances',
      workflowId: 'inventory_sync',
      mode: 'filter',
    });
    expect(block(saved, 'rate_limit_timeline').source).toMatchObject({
      kind: 'system',
      schema: '',
      entity: 'connection_rate_limit_timeline',
      interval: '24h',
      granularity: 'hourly',
    });
  });

  test('10 saves schema joins and custom source conditions', async ({
    page,
    mockApi,
  }) => {
    const condition = { op: 'EQ', arguments: ['status', 'open'] };
    const join = [
      {
        schema: 'customers',
        alias: 'customer',
        parentField: 'customer_id',
        field: 'id',
        op: 'eq',
        kind: 'left' as const,
      },
    ];
    const definition = baseDefinition([
      tableBlock('orders', {
        source: {
          schema: 'orders',
          mode: 'filter',
          join,
          condition,
        },
        table: {
          columns: [
            { field: 'id', label: 'ID' },
            { field: 'customer.name', label: 'Customer' },
          ],
        },
      }),
    ]);

    const { getSaved } = await openReportEditor({
      page,
      mockApi,
      id: 'joins_conditions',
      definition,
    });
    const saved = await saveThroughWizard(page, getSaved);

    expect(block(saved, 'orders').source.join).toEqual(join);
    expect(block(saved, 'orders').source.condition).toEqual(condition);
  });

  test('11 saves source orderBy, limit, interval, and time bucketing', async ({
    page,
    mockApi,
  }) => {
    const definition = baseDefinition([
      tableBlock('orders', {
        source: {
          schema: 'orders',
          mode: 'filter',
          orderBy: [{ field: 'amount', direction: 'desc' }],
          limit: 25,
          interval: '30d',
          granularity: 'daily',
        },
        table: {
          columns: [
            { field: 'id', label: 'ID' },
            { field: 'amount', label: 'Amount' },
          ],
        },
      }),
    ]);

    const { getSaved } = await openReportEditor({
      page,
      mockApi,
      id: 'source_query_controls',
      definition,
    });
    const saved = await saveThroughWizard(page, getSaved);

    expect(block(saved, 'orders').source).toMatchObject({
      orderBy: [{ field: 'amount', direction: 'desc' }],
      limit: 25,
      interval: '30d',
      granularity: 'daily',
    });
  });

  test('12 saves advanced aggregates', async ({ page, mockApi }) => {
    const definition = baseDefinition([
      {
        id: 'p90_amount',
        type: 'metric',
        title: 'P90 amount',
        source: {
          schema: 'orders',
          mode: 'aggregate',
          aggregates: [
            {
              alias: 'value',
              op: 'percentile_cont',
              field: 'amount',
              percentile: 0.9,
              distinct: true,
            },
          ],
        },
        metric: {
          valueField: 'value',
          label: 'P90 amount',
          format: 'currency',
        },
      },
      {
        id: 'status_total',
        type: 'chart',
        title: 'Status total',
        source: {
          schema: 'orders',
          mode: 'aggregate',
          groupBy: ['status'],
          aggregates: [{ alias: 'value', op: 'sum', field: 'amount' }],
        },
        chart: {
          kind: 'bar',
          x: 'status',
          series: [{ field: 'value', label: 'Total' }],
        },
      },
    ]);

    const { getSaved } = await openReportEditor({
      page,
      mockApi,
      id: 'advanced_aggregates',
      definition,
    });
    const saved = await saveThroughWizard(page, getSaved);

    expect(block(saved, 'p90_amount').source.aggregates?.[0]).toMatchObject({
      op: 'percentile_cont',
      field: 'amount',
      percentile: 0.9,
      distinct: true,
    });
    expect(block(saved, 'status_total').source.aggregates?.[0]).toMatchObject({
      op: 'sum',
      field: 'amount',
    });
  });

  test('13 saves deep card groups, subcards, subtables, and field kinds', async ({
    page,
    mockApi,
  }) => {
    const card = {
      groups: [
        {
          id: 'summary',
          title: 'Summary',
          fields: [
            { field: 'status', label: 'Status', format: 'pill' },
            {
              field: 'notes',
              label: 'Notes',
              kind: 'markdown' as const,
              collapsed: true,
            },
          ],
        },
        {
          id: 'details',
          title: 'Details',
          fields: [
            {
              field: 'customer',
              label: 'Customer',
              kind: 'subcard' as const,
              subcard: {
                groups: [
                  {
                    id: 'customer_summary',
                    fields: [
                      { field: 'customer_name', label: 'Name' },
                      { field: 'customer_email', label: 'Email' },
                    ],
                  },
                ],
              },
            },
            {
              field: 'line_items',
              label: 'Line items',
              kind: 'subtable' as const,
              subtable: {
                columns: [
                  { field: 'sku', label: 'SKU' },
                  { field: 'quantity', label: 'Qty', format: 'number' },
                ],
              },
            },
          ],
        },
      ],
    };
    const definition = baseDefinition([
      {
        id: 'order_card',
        type: 'card',
        title: 'Order card',
        source: { schema: 'orders', mode: 'filter' },
        card,
      },
    ]);

    const { getSaved } = await openReportEditor({
      page,
      mockApi,
      id: 'card_depth',
      definition,
    });
    const saved = await saveThroughWizard(page, getSaved);

    expect(block(saved, 'order_card').card).toEqual(card);
  });

  test('14 saves advanced filter option metadata', async ({
    page,
    mockApi,
  }) => {
    const filterMappings = [
      { filterId: 'country_filter', field: 'country_id', op: 'eq' },
    ];
    const optionsCondition = { op: 'EQ', arguments: ['active', true] };
    const definition = baseDefinition([tableBlock()], {
      filters: [
        {
          id: 'country_filter',
          label: 'Country',
          type: 'select',
          appliesTo: [{ field: 'country_id', op: 'eq' }],
        },
        {
          id: 'customer_filter',
          label: 'Customer',
          type: 'select',
          appliesTo: [{ blockId: 'orders', field: 'customer_id', op: 'eq' }],
          options: {
            source: 'object_model',
            schema: 'customers',
            field: 'id',
            valueField: 'id',
            labelField: 'name',
            dependsOn: ['country_filter'],
            filterMappings,
            condition: optionsCondition,
          },
        },
      ],
    });

    const { getSaved } = await openReportEditor({
      page,
      mockApi,
      id: 'advanced_filter_options',
      definition,
    });
    const saved = await saveThroughWizard(page, getSaved);
    const customerFilter = saved.filters.find(
      (filter) => filter.id === 'customer_filter'
    )!;

    expect(customerFilter.options).toMatchObject({
      source: 'object_model',
      schema: 'customers',
      field: 'id',
      valueField: 'id',
      labelField: 'name',
      search: true,
      dependsOn: ['country_filter'],
    });
    expect(customerFilter.options?.filterMappings).toEqual(filterMappings);
    expect(customerFilter.options?.condition).toEqual(optionsCondition);
  });

  test('15 saves multi-target filter appliesTo mappings', async ({
    page,
    mockApi,
  }) => {
    const appliesTo = [
      { blockId: 'orders', field: 'status', op: 'eq' },
      { blockId: 'order_summary', field: 'status', op: 'eq' },
    ];
    const definition = baseDefinition(
      [
        tableBlock('orders'),
        tableBlock('order_summary', {
          title: 'Order summary',
          table: {
            columns: [
              { field: 'status', label: 'Status' },
              { field: 'amount', label: 'Amount' },
            ],
          },
        }),
      ],
      {
        filters: [
          {
            id: 'status_filter',
            label: 'Status',
            type: 'select',
            appliesTo,
          },
        ],
      }
    );

    const { getSaved } = await openReportEditor({
      page,
      mockApi,
      id: 'multi_target_filters',
      definition,
    });
    const saved = await saveThroughWizard(page, getSaved);

    expect(saved.filters[0].appliesTo).toEqual(appliesTo);
  });

  test('16 saves column display modifiers', async ({ page, mockApi }) => {
    const definition = baseDefinition([
      tableBlock('orders', {
        table: {
          columns: [
            {
              field: 'customer_id',
              label: 'Customer',
              displayField: 'customer_name',
              displayTemplate: '{{customer_name}} (#{{customer_id}})',
              secondaryField: 'customer_email',
              linkField: 'customer_url',
              tooltipField: 'customer_email',
              align: 'center',
              maxChars: 24,
              descriptive: true,
            },
            {
              field: 'status',
              label: 'Status',
              format: 'bar_indicator',
              levels: ['queued', 'open', 'closed'],
            },
          ],
        },
      }),
    ]);

    const { getSaved } = await openReportEditor({
      page,
      mockApi,
      id: 'column_display_modifiers',
      definition,
    });
    const saved = await saveThroughWizard(page, getSaved);
    const columns = block(saved, 'orders').table?.columns ?? [];

    expect(columns[0]).toMatchObject({
      displayField: 'customer_name',
      displayTemplate: '{{customer_name}} (#{{customer_id}})',
      secondaryField: 'customer_email',
      linkField: 'customer_url',
      tooltipField: 'customer_email',
      align: 'center',
      maxChars: 24,
      descriptive: true,
    });
    expect(columns[1].levels).toEqual(['queued', 'open', 'closed']);
  });

  test('17 infers per-row workflow action and interaction button columns', async ({
    page,
    mockApi,
  }) => {
    const definition = baseDefinition([
      tableBlock('orders', {
        table: {
          columns: [
            { field: 'id', label: 'ID' },
            {
              field: 'run',
              label: 'Run',
              workflowAction: {
                workflowId: 'workflow_run',
                label: 'Run',
                context: { mode: 'row' },
              },
            },
            {
              field: 'open',
              label: 'Open',
              interactionButtons: [
                {
                  id: 'open_detail',
                  label: 'Open',
                  actions: [{ type: 'navigate_view', viewId: 'detail' }],
                },
              ],
            },
          ],
        },
      }),
    ]);

    const { getSaved } = await openReportEditor({
      page,
      mockApi,
      id: 'per_row_actions',
      definition,
    });
    const saved = await saveThroughWizard(page, getSaved);
    const columns = block(saved, 'orders').table?.columns ?? [];

    expect(columns[1]).toMatchObject({
      type: 'workflow_button',
      workflowAction: { workflowId: 'workflow_run' },
    });
    expect(columns[2]).toMatchObject({
      type: 'interaction_buttons',
      interactionButtons: [{ id: 'open_detail' }],
    });
  });

  test('18 renders viewer Print, Refresh, Explore, and full pagination', async ({
    page,
    mockApi,
  }) => {
    const definition = baseDefinition(
      [
        tableBlock('orders', {
          dataset: {
            id: 'order_totals',
            dimensions: ['status'],
            measures: ['count'],
          },
          table: {
            columns: [
              { field: 'status', label: 'Status' },
              { field: 'count', label: 'Count', format: 'number' },
            ],
            pagination: {
              defaultPageSize: 25,
              allowedPageSizes: [10, 25, 50],
            },
          },
        }),
      ],
      {
        datasets: [
          {
            id: 'order_totals',
            label: 'Order totals',
            source: { schema: 'orders' },
            dimensions: [{ field: 'status', label: 'Status', type: 'string' }],
            measures: [
              { id: 'count', label: 'Count', op: 'count', format: 'number' },
            ],
          },
        ],
      }
    );
    const report = reportFor('viewer_affordances', definition);
    const initialTable: ReportBlockResult = {
      type: 'table',
      status: 'ready',
      data: {
        columns: [
          { key: 'status', label: 'Status' },
          { key: 'count', label: 'Count', type: 'number' },
        ],
        rows: [{ status: 'open', count: 25 }],
        page: { offset: 0, size: 25, totalCount: 120, hasNextPage: true },
      },
    };
    let renderCalls = 0;
    const blockDataRequests: Array<Record<string, unknown>> = [];

    await page.addInitScript(() => {
      window.print = () => {
        (window as Window & { __syn410Printed?: boolean }).__syn410Printed =
          true;
      };
    });
    await mockApi.bootstrap(page);
    await mockApi.objects.schemas.list(page, SCHEMAS);
    await mockApi.raw(page, runtimeUrl('reports/viewer_affordances'), {
      report,
    });
    await mockApi.raw(
      page,
      runtimeUrl('reports/viewer_affordances/render'),
      async (route) => {
        renderCalls += 1;
        await fulfill(
          route,
          renderResponse('viewer_affordances', { orders: initialTable })
        );
      }
    );
    await mockApi.raw(
      page,
      runtimeUrl('reports/viewer_affordances/blocks/orders/data'),
      async (route) => {
        const request = route.request().postDataJSON() as Record<
          string,
          unknown
        >;
        blockDataRequests.push(request);
        const pageRequest =
          (request.page as { offset?: number; size?: number } | undefined) ??
          {};
        await fulfill(route, {
          ...initialTable,
          data: {
            ...(initialTable.data as Record<string, unknown>),
            rows: [{ status: 'closed', count: 20 }],
            page: {
              offset: pageRequest.offset ?? 0,
              size: pageRequest.size ?? 25,
              totalCount: 120,
              hasNextPage: false,
            },
          },
        });
      }
    );

    await gotoAppRoute(page, '/reports/viewer_affordances');
    await expect(page.getByRole('button', { name: /^Explore$/ })).toBeVisible();
    await expect(page.getByRole('button', { name: /^Print$/ })).toBeEnabled();
    await expect(page.getByRole('button', { name: /^Refresh$/ })).toBeEnabled();
    await expect(
      page.getByRole('button', { name: /^Explore this$/ })
    ).toBeVisible();
    await expect(page.getByText('Page 1 of 5')).toBeVisible();
    await expect(page.getByRole('button', { name: /First/ })).toBeVisible();
    await expect(page.getByRole('button', { name: /Previous/ })).toBeVisible();
    await expect(page.getByRole('button', { name: /Next/ })).toBeVisible();
    await expect(page.getByRole('button', { name: /Last/ })).toBeVisible();

    await page.getByRole('button', { name: /^Print$/ }).click();
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            (window as Window & { __syn410Printed?: boolean }).__syn410Printed
        )
      )
      .toBe(true);

    await page.getByRole('button', { name: /^Refresh$/ }).click();
    await expect.poll(() => renderCalls).toBeGreaterThanOrEqual(2);

    await page.getByRole('button', { name: /Last/ }).click();
    await expect
      .poll(() => blockDataRequests.at(-1)?.page)
      .toEqual({ offset: 100, size: 25 });
  });

  test('19 saves metric_row and columns layout primitives', async ({
    page,
    mockApi,
  }) => {
    const definition = baseDefinition(
      [
        {
          id: 'order_count',
          type: 'metric',
          title: 'Orders',
          source: {
            schema: 'orders',
            mode: 'aggregate',
            aggregates: [{ alias: 'value', op: 'count' }],
          },
          metric: { valueField: 'value', label: 'Orders' },
        },
        {
          id: 'total_amount',
          type: 'metric',
          title: 'Total',
          source: {
            schema: 'orders',
            mode: 'aggregate',
            aggregates: [{ alias: 'value', op: 'sum', field: 'amount' }],
          },
          metric: { valueField: 'value', label: 'Total' },
        },
        tableBlock('orders', {
          table: { columns: [{ field: 'id', label: 'ID' }] },
        }),
        {
          id: 'status_chart',
          type: 'chart',
          title: 'Status',
          source: {
            schema: 'orders',
            mode: 'aggregate',
            groupBy: ['status'],
            aggregates: [{ alias: 'value', op: 'count' }],
          },
          chart: { kind: 'bar', x: 'status', series: [{ field: 'value' }] },
        },
      ],
      {
        layout: [
          {
            id: 'metrics',
            type: 'metric_row',
            title: 'Metrics',
            blocks: ['order_count', 'total_amount'],
          },
          {
            id: 'split',
            type: 'columns',
            columns: [
              {
                id: 'left',
                children: [
                  { id: 'orders_node', type: 'block', blockId: 'orders' },
                ],
              },
              {
                id: 'right',
                children: [
                  { id: 'status_node', type: 'block', blockId: 'status_chart' },
                ],
              },
            ],
          },
        ],
      }
    );

    const { getSaved } = await openReportEditor({
      page,
      mockApi,
      id: 'layout_primitives',
      definition,
    });
    const saved = await saveThroughWizard(page, getSaved);

    expect(saved.layout?.[0]).toMatchObject({
      id: 'metrics',
      type: 'metric_row',
      blocks: ['order_count', 'total_amount'],
    });
    expect(saved.layout?.[1]).toMatchObject({
      id: 'split',
      type: 'columns',
    });
    expect(
      saved.layout?.[1].type === 'columns'
        ? saved.layout[1].columns.flatMap((column) =>
            (column.children ?? []).map((child) =>
              child.type === 'block' ? child.blockId : null
            )
          )
        : []
    ).toEqual(['orders', 'status_chart']);
  });

  test('20 saves actions block type', async ({ page, mockApi }) => {
    const definition = baseDefinition([
      {
        id: 'open_actions',
        type: 'actions',
        title: 'Open actions',
        source: {
          kind: 'workflow_runtime',
          schema: '',
          entity: 'actions',
          workflowId: 'inventory_sync',
          mode: 'filter',
        },
        actions: {
          submit: {
            label: 'Resolve action',
            implicitPayload: { resolution: 'approved' },
          },
        },
      },
    ]);

    const { getSaved } = await openReportEditor({
      page,
      mockApi,
      id: 'actions_block',
      definition,
    });
    const saved = await saveThroughWizard(page, getSaved);
    const actionsBlock = block(saved, 'open_actions');

    expect(actionsBlock).toMatchObject({
      id: 'open_actions',
      type: 'actions',
      actions: {
        submit: {
          label: 'Resolve action',
        },
      },
    });
    expect(actionsBlock.source).toMatchObject({
      kind: 'workflow_runtime',
      schema: '',
      entity: 'actions',
      workflowId: 'inventory_sync',
      mode: 'filter',
    });
  });
});
