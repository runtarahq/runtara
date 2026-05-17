// Phase 9 dnd-kit follow-up: drive a real drag-and-drop reorder
// through the wizard grid editor and assert the persisted layout
// reflects the new order. The dnd-kit PointerSensor activates after
// the pointer moves 4px, so we use real mouse.down/move/up rather
// than Playwright's higher-level `dragTo`.
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

const TENANT = 'tenant_wizard_v2_dnd';
const REPORT_ID = 'rep_wizard_v2_dnd';

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
  columns: [{ name: 'order_id', type: 'string' }],
} as Schema;

function reportWithGrid(): ReportDto {
  const definition: ReportDefinition = {
    definitionVersion: 1,
    filters: [],
    blocks: [
      {
        id: 'a',
        type: 'markdown',
        title: 'A',
        source: { schema: '' },
        markdown: { content: '# A' },
      },
      {
        id: 'b',
        type: 'markdown',
        title: 'B',
        source: { schema: '' },
        markdown: { content: '# B' },
      },
      {
        id: 'c',
        type: 'markdown',
        title: 'C',
        source: { schema: '' },
        markdown: { content: '# C' },
      },
    ],
    layout: [
      {
        id: 'g_root',
        type: 'grid',
        columns: 1,
        items: [
          {
            id: 'item_a',
            child: { id: 'n_a', type: 'block', blockId: 'a' },
          },
          {
            id: 'item_b',
            child: { id: 'n_b', type: 'block', blockId: 'b' },
          },
          {
            id: 'item_c',
            child: { id: 'n_c', type: 'block', blockId: 'c' },
          },
        ],
      },
    ],
  };
  return {
    id: REPORT_ID,
    slug: 'grid-dnd',
    name: 'Grid DnD',
    description: null,
    tags: [],
    status: 'published',
    definitionVersion: 1,
    definition,
    createdAt: '2026-05-17T00:00:00Z',
    updatedAt: '2026-05-17T00:00:00Z',
  };
}

async function setup(
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
      const updated: ReportDto = {
        ...reportWithGrid(),
        name: saved!.name,
        description: saved!.description ?? null,
        tags: saved!.tags,
        status: saved!.status,
        definitionVersion: saved!.definition.definitionVersion,
        definition: saved!.definition,
        updatedAt: '2026-05-17T00:01:00Z',
      };
      await fulfill(route, { report: updated });
      return;
    }
    await fulfill(route, { report: reportWithGrid() });
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

test.describe('wizard v2 grid drag-and-drop (mocked)', () => {
  test('dragging the first block onto the third reorders to [b, c, a]', async ({
    page,
    mockApi,
  }) => {
    const { getSaved } = await setup(
      page,
      mockApi as unknown as import('../../../fixtures/mock.fixture').MockApi
    );

    // The grid renders three block cards. Each block has a draggable
    // grip button ('Drag block'). Grab the grip on block A and drop it
    // on block C's card.
    const grips = page.getByRole('button', { name: 'Drag block' });
    await expect(grips).toHaveCount(3);

    const aBox = await grips.nth(0).boundingBox();
    const cCard = page.locator('[data-block-id="c"]');
    const cBox = await cCard.boundingBox();
    if (!aBox || !cBox) throw new Error('expected bounding boxes');

    // dnd-kit PointerSensor activates after 4px. Use slow incremental
    // mouse moves so the activation kicks in. The grip on block A is
    // the source; the body of block C is the destination.
    await grips.nth(0).hover();
    await page.mouse.down();
    // Nudge past the 4px activation distance.
    await page.mouse.move(
      aBox.x + aBox.width / 2 + 10,
      aBox.y + aBox.height / 2,
      { steps: 5 }
    );
    // Move into C's card with many intermediate steps so dnd-kit's
    // collision detection updates `over` along the way.
    await page.mouse.move(
      cBox.x + cBox.width / 2,
      cBox.y + cBox.height / 2,
      { steps: 25 }
    );
    // Settle on the target before releasing.
    await page.waitForTimeout(50);
    await page.mouse.up();
    // Give React state a tick to commit the move before clicking Save.
    await page.waitForTimeout(100);

    // Sanity-check the visual order matches expectations before save.
    const blockOrderAfterDrag = await page
      .locator('[data-block-id]')
      .evaluateAll((els) => els.map((el) => el.getAttribute('data-block-id')));
    expect(blockOrderAfterDrag).toEqual(['b', 'c', 'a']);

    await page.getByRole('button', { name: /^Save$/ }).click();

    await expect(async () => {
      expect(getSaved()).not.toBeNull();
    }).toPass({ timeout: 5000 });

    const saved = getSaved()!;
    const root = saved.definition.layout?.[0];
    expect(root?.type).toBe('grid');
    if (root?.type !== 'grid') return;
    const order = root.items.map((item) =>
      item.child.type === 'block' ? item.child.blockId : item.child.id
    );
    expect(order).toEqual(['b', 'c', 'a']);
  });
});
