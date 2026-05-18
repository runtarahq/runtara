// Phase 9/10 acceptance E2E: drive the root-grid layout editor through a
// realistic author flow — open an existing empty report in edit mode,
// drop a block into the root grid, save, assert the persisted definition
// has the block as an item of the root grid + a matching block on the
// blocks array. Phase 10 made the root layout a single mandatory grid;
// authors no longer add floating siblings at the report root.
import type { Page, Route } from '@playwright/test';
import {
  buildObjectModelConnection,
  expect,
  test,
} from '../../../fixtures';
import { appPath } from '../../../utils/app-path';
import type { Schema } from '../../../../src/generated/RuntaraRuntimeApi';
import type {
  ReportDefinition,
  ReportDto,
  UpdateReportRequest,
} from '../../../../src/features/reports/types';

const TENANT = 'tenant_wizard_v2_grid';
const REPORT_ID = 'rep_wizard_v2_grid';

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

const SCHEMA: Schema = {
  id: 'Order',
  name: 'Order',
  tableName: 'orders',
  tenantId: TENANT,
  createdAt: '2026-05-15T00:00:00Z',
  updatedAt: '2026-05-15T00:00:00Z',
  columns: [
    { name: 'order_id', type: 'string' },
    { name: 'status', type: 'string' },
  ],
} as Schema;

function emptyReport(): ReportDto {
  return {
    id: REPORT_ID,
    slug: 'grid-flow',
    name: 'Grid flow',
    description: null,
    tags: [],
    status: 'published',
    definitionVersion: 1,
    definition: {
      definitionVersion: 1,
      layout: { id: 'root', columns: 1, rows: 1, items: [] },
      filters: [],
      blocks: [],
    },
    createdAt: '2026-05-17T00:00:00Z',
    updatedAt: '2026-05-17T00:00:00Z',
  };
}

async function setupGridEditing(
  page: Page,
  mockApi: import('../../../fixtures/mock.fixture').MockApi
): Promise<{ getSaved: () => UpdateReportRequest | null }> {
  await mockApi.bootstrap(page);
  await mockApi.connections.list(page, [
    buildObjectModelConnection({ id: 'conn_object_model_default' }),
  ]);
  await mockApi.objects.schemas.list(page, [SCHEMA]);

  let saved: UpdateReportRequest | null = null;
  await mockApi.raw(page, runtimeUrl(`reports/${REPORT_ID}`), async (route) => {
    if (route.request().method() === 'PUT') {
      saved = JSON.parse(
        route.request().postData() ?? '{}'
      ) as UpdateReportRequest;
      const definition = saved!.definition;
      const updated: ReportDto = {
        ...emptyReport(),
        name: saved!.name,
        description: saved!.description ?? null,
        tags: saved!.tags,
        status: saved!.status,
        definitionVersion: definition.definitionVersion,
        definition,
        updatedAt: '2026-05-17T00:01:00Z',
      };
      await fulfill(route, { report: updated });
      return;
    }
    await fulfill(route, { report: emptyReport() });
  });

  await mockApi.raw(page, runtimeUrl('reports/validate'), {
    valid: true,
    errors: [],
    warnings: [],
  });
  await mockApi.raw(page, runtimeUrl('reports/preview'), {
    success: true,
    report: { id: REPORT_ID, definitionVersion: 1 },
    resolvedFilters: {},
    blocks: {},
    errors: [],
  });

  await gotoAppRoute(page, `/reports/${REPORT_ID}?edit=1`);
  await expect(page.getByRole('button', { name: /^Save$/ })).toBeVisible();

  return { getSaved: () => saved };
}

test.describe('wizard v2 grid layout author flow (mocked)', () => {
  test('root grid is always visible — even on a brand-new empty report', async ({
    page,
    mockApi,
  }) => {
    await setupGridEditing(
      page,
      mockApi as unknown as import('../../../fixtures/mock.fixture').MockApi
    );
    await expect(
      page.getByRole('heading', { name: 'Layout', level: 2 })
    ).toBeVisible();
    // Root grid header label reads "Report layout · 1×1" for empty reports.
    await expect(page.getByText(/Report layout · 1×1/)).toBeVisible();
    // No "Remove grid" button on the root grid — it cannot be removed.
    await expect(
      page.getByRole('button', { name: 'Remove grid' })
    ).toHaveCount(0);
  });

  test('add a block into the root grid → save persists it as a root grid item', async ({
    page,
    mockApi,
  }) => {
    const { getSaved } = await setupGridEditing(
      page,
      mockApi as unknown as import('../../../fixtures/mock.fixture').MockApi
    );

    // Click an empty cell's "+ Add block" button.
    await page
      .getByRole('button', { name: /^Add block$/i })
      .first()
      .click();

    // Save.
    await page.getByRole('button', { name: /^Save$/ }).click();

    await expect(async () => {
      expect(getSaved()).not.toBeNull();
    }).toPass({ timeout: 5000 });

    const saved = getSaved()!;
    const definition: ReportDefinition = saved.definition;

    // Root grid present, with a single item pointing at the new block.
    expect(definition.layout.id).toBe('root');
    expect(definition.layout.items).toHaveLength(1);
    const child = definition.layout.items[0].child;
    expect(child.type).toBe('block');
    if (child.type !== 'block') return;
    const matchingBlock = definition.blocks.find(
      (b) => b.id === child.blockId
    );
    expect(matchingBlock).toBeDefined();
    expect(matchingBlock?.type).toBe('markdown');
  });

  test('inline columns/rows steppers grow the root grid skeleton + persist on save', async ({
    page,
    mockApi,
  }) => {
    const { getSaved } = await setupGridEditing(
      page,
      mockApi as unknown as import('../../../fixtures/mock.fixture').MockApi
    );

    // Bump columns to 3.
    await page.getByLabel('Add columns').click();
    await page.getByLabel('Add columns').click();
    await expect(page.getByText(/Report layout · 3×1/)).toBeVisible();

    // Bump rows to 2.
    await page.getByLabel('Add rows').click();
    await expect(page.getByText(/Report layout · 3×2/)).toBeVisible();

    // 6 empty cells visible (3 cols × 2 rows, no items yet) in the root grid.
    const emptyCells = page.getByTestId('empty-cell-root');
    await expect(emptyCells).toHaveCount(6);

    // Save.
    await page.getByRole('button', { name: /^Save$/ }).click();
    await expect(async () => {
      expect(getSaved()).not.toBeNull();
    }).toPass({ timeout: 5000 });

    const saved = getSaved()!;
    expect(saved.definition.layout.id).toBe('root');
    expect(saved.definition.layout.columns).toBe(3);
    expect(saved.definition.layout.rows).toBe(2);
  });

  test('clicking the bottom-right empty cell of a 3×2 grid pins the new block to that exact cell', async ({
    page,
    mockApi,
  }) => {
    const { getSaved } = await setupGridEditing(
      page,
      mockApi as unknown as import('../../../fixtures/mock.fixture').MockApi
    );

    // Grow root to 3×2.
    await page.getByLabel('Add columns').click();
    await page.getByLabel('Add columns').click();
    await page.getByLabel('Add rows').click();
    await expect(page.getByText(/Report layout · 3×2/)).toBeVisible();

    // The 6th empty cell (row-major) is the bottom-right one.
    const emptyCells = page.getByTestId('empty-cell-root');
    await expect(emptyCells).toHaveCount(6);
    // Click the "+ Add block" inside the last empty cell.
    await emptyCells.nth(5).getByRole('button', { name: /Add block/i }).click();

    // The inline editor should open immediately for the new block.
    await expect(
      page.locator('[data-testid^="inline-editor-"]')
    ).toBeVisible();

    // Dismiss the inline editor (Done) and save.
    await page.getByRole('button', { name: /^Done$/ }).click();
    await page.getByRole('button', { name: /^Save$/ }).click();
    await expect(async () => {
      expect(getSaved()).not.toBeNull();
    }).toPass({ timeout: 5000 });

    const saved = getSaved()!;
    expect(saved.definition.layout.items).toHaveLength(1);
    const item = saved.definition.layout.items[0];
    expect(item.col).toBe(3);
    expect(item.row).toBe(2);
  });
});
