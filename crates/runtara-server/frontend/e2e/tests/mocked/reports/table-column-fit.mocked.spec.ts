import type { Page, Route } from '@playwright/test';
import {
  buildObjectModelConnection,
  expect,
  test,
  type MockApi,
} from '../../../fixtures';
import { appPath } from '../../../utils/app-path';
import type {
  ReportDefinition,
  ReportDto,
  ReportRenderResponse,
} from '../../../../src/features/reports/types';

const FIT_REPORT_ID = 'report_table_column_fit';
const RIGID_REPORT_ID = 'report_table_rigid_overflow';

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

function reportFor(id: string, definition: ReportDefinition): ReportDto {
  return {
    id,
    slug: id,
    name: 'Table column fit',
    description: null,
    tags: [],
    status: 'published',
    definitionVersion: 1,
    definition,
    createdAt: '2026-07-21T00:00:00Z',
    updatedAt: '2026-07-21T00:00:00Z',
  };
}

function tableDefinition(
  blockId: string,
  columns: Array<Record<string, unknown>>
): ReportDefinition {
  return {
    definitionVersion: 1,
    layout: {
      id: 'root',
      columns: 1,
      items: [
        {
          id: 'root_item',
          child: { id: 'root_node', type: 'block', blockId },
        },
      ],
    },
    filters: [],
    blocks: [
      {
        id: blockId,
        type: 'table',
        title: 'Records',
        source: { schema: 'FitDemoRecord', mode: 'filter' },
        table: { columns },
      } as never,
    ],
  };
}

async function bootstrapReport(
  page: Page,
  mockApi: MockApi,
  reportId: string,
  definition: ReportDefinition,
  render: ReportRenderResponse
) {
  await mockApi.bootstrap(page);
  await mockApi.connections.list(page, [
    buildObjectModelConnection({ id: 'conn_object_model_default' }),
  ]);
  await mockApi.objects.schemas.list(page, []);
  await mockApi.raw(page, runtimeUrl(`reports/${reportId}`), {
    report: reportFor(reportId, definition),
  });
  await mockApi.raw(
    page,
    runtimeUrl(`reports/${reportId}/render`),
    async (route) => {
      await fulfill(route, render);
    }
  );
}

type TableMetrics = {
  scrollportWidth: number;
  tableWidth: number;
  overflow: number;
  colWidths: number[];
};

async function tableMetrics(page: Page): Promise<TableMetrics> {
  return page.evaluate(() => {
    const table = document.querySelector('table.table-fixed');
    if (!table) throw new Error('table not rendered');
    // The Table primitive wraps the <table> in its own overflow-auto div —
    // that inner div is the real horizontal scrollport.
    const scrollport = table.parentElement as HTMLElement;
    return {
      scrollportWidth: scrollport.clientWidth,
      tableWidth: Math.round(table.getBoundingClientRect().width),
      overflow: scrollport.scrollWidth - scrollport.clientWidth,
      colWidths: Array.from(table.querySelectorAll('col'))
        .map((col) => Number.parseFloat((col as HTMLElement).style.width))
        .filter((width) => Number.isFinite(width)),
    };
  });
}

const LONG_TEXT_A =
  'A long narrative summary that would push this column to its maximum width';
const LONG_TEXT_B =
  'Another verbose description with plenty of words to overflow the space';
const LONG_TEXT_C =
  'Rationale text produced by an AI agent explaining the score in detail';

test.describe('report table column fit (mocked)', () => {
  test('flexible columns compress to the container instead of forcing horizontal scroll', async ({
    page,
    mockApi,
  }) => {
    const blockId = 'fit_table';
    const definition = tableDefinition(blockId, [
      { field: 'summary', label: 'Summary' },
      { field: 'description', label: 'Description' },
      { field: 'rationale', label: 'Rationale' },
      { field: 'amount', label: 'Amount', format: 'currency' },
      { field: 'balance', label: 'Balance', format: 'currency' },
      { field: 'status', label: 'Status', format: 'pill' },
    ]);
    const render: ReportRenderResponse = {
      success: true,
      report: { id: FIT_REPORT_ID, definitionVersion: 1 },
      resolvedFilters: {},
      blocks: {
        [blockId]: {
          type: 'table',
          status: 'ready',
          data: {
            columns: [
              { key: 'summary', label: 'Summary' },
              { key: 'description', label: 'Description' },
              { key: 'rationale', label: 'Rationale' },
              { key: 'amount', label: 'Amount', format: 'currency' },
              { key: 'balance', label: 'Balance', format: 'currency' },
              { key: 'status', label: 'Status', format: 'pill' },
            ],
            rows: [
              {
                id: 'row-1',
                summary: LONG_TEXT_A,
                description: LONG_TEXT_B,
                rationale: LONG_TEXT_C,
                amount: 1520.55,
                balance: 98341.02,
                status: 'in_progress',
              },
              {
                id: 'row-2',
                summary: LONG_TEXT_B,
                description: LONG_TEXT_C,
                rationale: LONG_TEXT_A,
                amount: 12.4,
                balance: 20.99,
                status: 'done',
              },
            ],
          },
        },
      },
      errors: [],
    };

    await bootstrapReport(page, mockApi as MockApi, FIT_REPORT_ID, definition, render);
    await page.goto(appPath(`/reports/${FIT_REPORT_ID}`));
    await expect(page.locator('table.table-fixed')).toBeVisible();

    // Narrow the viewport into the compression regime: the ideal widths
    // (three ~259px text columns + rigid columns ≈ 1100px) no longer fit,
    // but the column minimums still do. This is the regression case that
    // used to scroll horizontally.
    await page.setViewportSize({ width: 1050, height: 800 });

    await expect
      .poll(async () => (await tableMetrics(page)).overflow, {
        message: 'flexible columns should compress until the table fits',
      })
      .toBeLessThanOrEqual(1);

    const metrics = await tableMetrics(page);
    expect(metrics.tableWidth).toBeLessThanOrEqual(metrics.scrollportWidth + 1);
    const colSum = metrics.colWidths.reduce((acc, w) => acc + w, 0);
    expect(colSum).toBeLessThanOrEqual(metrics.scrollportWidth + 1);
    // Compression actually happened: at least one text column sits below its
    // ~259px ideal cap.
    expect(Math.min(...metrics.colWidths)).toBeLessThan(259);
    expect(metrics.colWidths.some((w) => w < 259 && w >= 97)).toBe(true);

    // Content still renders (truncated, not dropped).
    await expect(
      page.getByText('A long narrative summary', { exact: false }).first()
    ).toBeVisible();
  });

  test('a table of rigid columns still scrolls when it genuinely cannot fit', async ({
    page,
    mockApi,
  }) => {
    const blockId = 'rigid_table';
    const fields = Array.from({ length: 10 }, (_, i) => `metric_${i + 1}`);
    const definition = tableDefinition(
      blockId,
      fields.map((field, i) => ({
        field,
        label: `Metric ${i + 1}`,
        format: 'currency',
      }))
    );
    const row: Record<string, unknown> = { id: 'row-1' };
    for (const field of fields) row[field] = 123456.78;
    const render: ReportRenderResponse = {
      success: true,
      report: { id: RIGID_REPORT_ID, definitionVersion: 1 },
      resolvedFilters: {},
      blocks: {
        [blockId]: {
          type: 'table',
          status: 'ready',
          data: {
            columns: fields.map((field, i) => ({
              key: field,
              label: `Metric ${i + 1}`,
              format: 'currency',
            })),
            rows: [row],
          },
        },
      },
      errors: [],
    };

    await bootstrapReport(
      page,
      mockApi as MockApi,
      RIGID_REPORT_ID,
      definition,
      render
    );
    await page.goto(appPath(`/reports/${RIGID_REPORT_ID}`));
    await expect(page.locator('table.table-fixed')).toBeVisible();

    await page.setViewportSize({ width: 900, height: 800 });

    // Ten currency columns can't compress (numbers never truncate) — the
    // table must overflow into horizontal scroll rather than corrupt values.
    await expect
      .poll(async () => (await tableMetrics(page)).overflow)
      .toBeGreaterThan(50);
  });
});
