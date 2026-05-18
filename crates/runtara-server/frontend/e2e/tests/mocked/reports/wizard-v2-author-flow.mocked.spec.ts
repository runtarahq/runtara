// Phase 7 cutover regression: drive the wizard v2 through an author
// flow (open an existing empty report in edit mode → add a markdown
// block → edit content → save) and assert the persisted
// `ReportDefinition` captures every edit. Also asserts the wizard v2 is
// wired as the default authoring surface — the legacy wizard was
// deleted in this same phase, so this test would fail loudly if v2
// regressed.
//
// All API calls are intercepted; no real backend.
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

const TENANT = 'tenant_wizard_v2';
const REPORT_ID = 'rep_wizard_v2_test';

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
    { name: 'total_amount', type: 'number' },
  ],
} as Schema;

function emptyReport(): ReportDto {
  return {
    id: REPORT_ID,
    slug: 'wizard-v2-test',
    name: 'Wizard v2 test',
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

async function setupWizardEditing(
  page: Page,
  mockApi: typeof import('../../../fixtures')['test']['_mockApi'] extends never
    ? never
    : import('../../../fixtures/mock.fixture').MockApi
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

  await gotoAppRoute(page, `/reports/${REPORT_ID}?edit=1`);
  await expect(page.getByRole('button', { name: /^Save$/ })).toBeVisible();

  return { getSaved: () => saved };
}

test.describe('wizard v2 author flow (mocked)', () => {
  test('default surface is wizard v2 — Layout section header renders (grid-only model)', async ({
    page,
    mockApi,
  }) => {
    await setupWizardEditing(
      page,
      mockApi as unknown as Parameters<typeof setupWizardEditing>[1]
    );

    await expect(
      page.getByRole('heading', { name: 'Layout', level: 2 })
    ).toBeVisible();
    await expect(page.getByText(/everything is a grid/)).toBeVisible();
    // No legacy compatibility warning anywhere on the page.
    await expect(
      page.getByText(/This report uses advanced features/)
    ).toHaveCount(0);
  });

  test('add a block into the root grid, save → PUT captures the structure', async ({
    page,
    mockApi,
  }) => {
    const { getSaved } = await setupWizardEditing(
      page,
      mockApi as unknown as Parameters<typeof setupWizardEditing>[1]
    );

    // Phase 10: the root grid renders an empty cell with a "+ Add block"
    // affordance. Clicking it pushes a fresh markdown block onto
    // `definition.blocks` and adds a matching grid item under the root.
    await page
      .getByRole('button', { name: /^Add block$/i })
      .first()
      .click();

    await page.getByRole('button', { name: /^Save$/ }).click();

    await expect(async () => {
      expect(getSaved()).not.toBeNull();
    }).toPass({ timeout: 5000 });

    const saved = getSaved()!;
    const definition: ReportDefinition = saved.definition;
    expect(definition.blocks).toHaveLength(1);
    const block = definition.blocks[0];
    expect(block.type).toBe('markdown');
    // The block appears as a child of an item under the root grid.
    expect(
      (definition.layout.items ?? []).some(
        (item) => item.child.type === 'block' && item.child.blockId === block.id
      )
    ).toBe(true);
  });
});
