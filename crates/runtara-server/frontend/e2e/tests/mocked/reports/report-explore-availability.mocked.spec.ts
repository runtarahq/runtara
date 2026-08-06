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

const PLAIN_REPORT_ID = 'report_explore_without_dataset';
const DATASET_REPORT_ID = 'report_explore_with_dataset';

const summaryBlock = {
  id: 'summary',
  type: 'markdown' as const,
  title: 'Summary',
  source: { schema: '', mode: 'filter' as const },
  markdown: { content: '# Summary' },
};

const baseDefinition: ReportDefinition = {
  definitionVersion: 1,
  layout: {
    id: 'root',
    columns: 1,
    items: [
      {
        id: 'root_i0',
        child: { id: 'n_summary', type: 'block', blockId: summaryBlock.id },
      },
    ],
  },
  filters: [],
  blocks: [summaryBlock],
};

const datasetDefinition: ReportDefinition = {
  ...baseDefinition,
  datasets: [
    {
      id: 'orders_ds',
      label: 'Orders',
      source: { schema: 'orders' },
      dimensions: [{ field: 'status', label: 'Status', type: 'string' }],
      measures: [
        { id: 'total', label: 'Total', op: 'count', format: 'number' },
      ],
    },
  ],
};

function runtimeUrl(suffix: string): RegExp {
  const escaped = suffix.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return new RegExp(`/api/runtime(?:/[^/]+)?/${escaped}(?:\\?[^/]*)?$`);
}

function reportFor(id: string, definition: ReportDefinition): ReportDto {
  return {
    id,
    slug: id,
    name: 'Explore availability',
    description: null,
    tags: [],
    status: 'published',
    definitionVersion: 1,
    definition,
    createdAt: '2026-07-27T00:00:00Z',
    updatedAt: '2026-07-27T00:00:00Z',
  };
}

function renderFor(id: string): ReportRenderResponse {
  return {
    success: true,
    report: { id, definitionVersion: 1 },
    resolvedFilters: {},
    blocks: {
      [summaryBlock.id]: {
        type: 'markdown',
        status: 'ready',
        data: { content: '# Summary' },
      },
    },
    errors: [],
  };
}

async function bootstrapReport(
  page: Page,
  mockApi: MockApi,
  reportId: string,
  definition: ReportDefinition
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
    async (route: Route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify(renderFor(reportId)),
      });
    }
  );
}

test.describe('report Explore availability (mocked)', () => {
  test('withholds the Explore action from a report with no semantic dataset', async ({
    page,
    mockApi,
  }) => {
    await bootstrapReport(page, mockApi, PLAIN_REPORT_ID, baseDefinition);
    await page.goto(appPath(`/reports/${PLAIN_REPORT_ID}`));

    // The header renders — so absence below is a real gate, not a slow load.
    await expect(page.getByRole('button', { name: 'Print' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Explore' })).toHaveCount(0);
  });

  test('offers the Explore action once the report exposes a dataset', async ({
    page,
    mockApi,
  }) => {
    await bootstrapReport(page, mockApi, DATASET_REPORT_ID, datasetDefinition);
    await page.goto(appPath(`/reports/${DATASET_REPORT_ID}`));

    await expect(page.getByRole('button', { name: 'Explore' })).toBeVisible();
    // The link carries the current search params through to Explore, so match
    // the path rather than the whole href.
    await expect(page.getByRole('link', { name: 'Explore' })).toHaveAttribute(
      'href',
      new RegExp(`/reports/${DATASET_REPORT_ID}/explore(\\?|$)`)
    );
  });

  test('keeps the unavailable message for a directly opened Explore URL', async ({
    page,
    mockApi,
  }) => {
    // Bookmarks and hand-typed URLs still reach the page, so the fallback
    // stays — hiding the action removes the dead end, not the safety net.
    await bootstrapReport(page, mockApi, PLAIN_REPORT_ID, baseDefinition);
    await page.goto(appPath(`/reports/${PLAIN_REPORT_ID}/explore`));

    await expect(
      page.getByText(
        'This report does not expose a semantic dataset for Explore.'
      )
    ).toBeVisible();
  });
});
