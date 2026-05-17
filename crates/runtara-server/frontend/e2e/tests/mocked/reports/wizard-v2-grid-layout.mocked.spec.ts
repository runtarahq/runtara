// Phase 9 acceptance E2E: drive the grid-only layout editor through a
// realistic author flow — open an existing empty report in edit mode,
// add a 2-column grid, add a block inside the grid, save, assert the
// persisted definition contains exactly one top-level grid with one
// block child plus a matching block on the blocks array.
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
      layout: [],
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
  // Preview API is debounced 400ms; fulfill with an empty preview so the
  // editor's BlockHostInEdit renders the placeholder rather than hanging.
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
  test('Layout section header renders + has the canonical grid-only copy', async ({
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
    await expect(page.getByText(/everything is a grid/)).toBeVisible();
  });

  test('add a 2-column grid + one block inside → save persists the structure', async ({
    page,
    mockApi,
  }) => {
    const { getSaved } = await setupGridEditing(
      page,
      mockApi as unknown as import('../../../fixtures/mock.fixture').MockApi
    );

    // Empty state — root-level "Add grid" dropdown.
    await page
      .getByRole('button', { name: /^Add grid$/i })
      .first()
      .click();
    await page.getByText('2 equal columns').click();

    // The new grid container now shows up with its own "Add block".
    // The grid-scoped affordance renders before the root-level dock, so
    // `.first()` reliably targets it.
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

    // Exactly one top-level grid node.
    expect(definition.layout).toHaveLength(1);
    const root = definition.layout?.[0];
    expect(root?.type).toBe('grid');
    if (root?.type !== 'grid') return;
    expect(root.columns).toBe(2);
    // One block sitting inside the grid.
    expect(root.items).toHaveLength(1);
    const item = root.items[0];
    expect(item.child.type).toBe('block');
    if (item.child.type !== 'block') return;
    // The block reference resolves to a real block on the blocks array.
    const matchingBlock = definition.blocks.find(
      (b) => b.id === item.child.blockId
    );
    expect(matchingBlock).toBeDefined();
    expect(matchingBlock?.type).toBe('markdown');
  });
});
