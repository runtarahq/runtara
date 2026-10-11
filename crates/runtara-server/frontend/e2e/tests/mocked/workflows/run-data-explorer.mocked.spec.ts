import { expect, type Page } from '@playwright/test';
import { test, buildWorkflow, type MockApi } from '../../../fixtures';
import { WorkflowHistoryPage } from '../../../pages/WorkflowExtraPages';

/**
 * The run page's data explorer against payloads well past 1 MB, in a real
 * browser: layout-driven scrolling, search reveal, the text-view size cap and
 * loading a server-elided value in full.
 */
test.describe('Run data explorer (mocked)', () => {
  const workflow = buildWorkflow({
    id: 'scn_big',
    name: 'Big payload workflow',
  });
  const instanceId = 'inst_big';
  const ITEMS = 15_000;

  const output = {
    feeds_seen: ITEMS,
    enriched: Array.from({ length: ITEMS }, (_, i) => ({
      sku: `SKU-${String(i).padStart(6, '0')}`,
      title: `Industrial part ${i} with a reasonably long product description`,
      market_price: { min: 1.5, median: 2.25, max: 3.75, currency: 'EUR' },
      keywords: ['part', `kw-${i % 37}`],
    })),
  };
  const fullUpload = 'JVBERi0xLjcK'.repeat(2_000);
  const elidedInputs = {
    data: {
      catalogId: 'cat_eu_2026',
      upload: {
        _elided: true,
        _original_size: 1_843_200,
        _preview: 'JVBERi0x',
      },
    },
    variables: {},
  };

  async function mockRun(page: Page, mockApi: MockApi) {
    await mockApi.bootstrap(page);
    await mockApi.workflows.get(page, workflow.id, workflow);
    const instance = {
      id: instanceId,
      workflowId: workflow.id,
      status: 'COMPLETED',
      usedVersion: 1,
      created: '2026-10-11T09:02:36Z',
      updated: '2026-10-11T09:02:40Z',
      inputs: elidedInputs,
      outputs: output,
    };
    await mockApi.workflows.instanceDetail(
      page,
      workflow.id,
      instanceId,
      instance,
      {
        workflowName: workflow.name,
      }
    );
    // Registered later, so it wins for the `?full=true` re-fetch.
    await page.route(/\/instances\/inst_big\?(?:.*&)?full=true/, (route) =>
      route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          success: true,
          data: {
            instance: {
              ...instance,
              inputs: {
                ...elidedInputs,
                data: { ...elidedInputs.data, upload: fullUpload },
              },
            },
            metadata: { workflowName: workflow.name },
          },
        }),
      })
    );
    await mockApi.workflows.stepSummaries(page, workflow.id, instanceId, []);
    await mockApi.workflows.stepEvents(page, workflow.id, instanceId, []);
  }

  test.beforeEach(async ({ page, mockApi }) => {
    expect(JSON.stringify(output).length).toBeGreaterThan(2_500_000);
    await mockRun(page, mockApi);
    await new WorkflowHistoryPage(page, workflow.id, instanceId).goto();
  });

  test('browses a multi-megabyte output with only a window of rows in the DOM', async ({
    page,
  }) => {
    const explorer = page.getByRole('region', { name: 'Run data' });
    await expect(explorer.getByTitle('Approximate JSON size')).toHaveText(
      /~\d\.\d MB/
    );

    const tree = explorer.getByRole('tree', { name: 'Output fields' });
    await tree
      .getByRole('treeitem')
      .filter({ hasText: 'enriched' })
      .getByRole('button', { name: 'Expand' })
      .click();
    // 15,000 items open as nested groups, not 15,000 rows.
    await expect(tree.locator('[data-row-id="enriched#0-9999"]')).toBeVisible();

    await explorer.getByRole('button', { name: 'Expand all' }).click();
    const rendered = await tree.getByRole('treeitem').count();
    expect(rendered).toBeLessThan(50);

    // Scrolling far down renders rows from deep in the list.
    await tree.evaluate((el) => el.scrollTo({ top: 40_000 }));
    await expect
      .poll(async () =>
        tree.getByRole('treeitem').first().getAttribute('data-row-id')
      )
      .not.toBe('feeds_seen');
    expect(await tree.getByRole('treeitem').count()).toBeLessThan(50);
  });

  test('finds a deep match quickly and scrolls it into view', async ({
    page,
  }) => {
    const explorer = page.getByRole('region', { name: 'Run data' });
    const started = Date.now();
    await explorer
      .getByRole('searchbox', { name: 'Search keys and values' })
      .fill('SKU-014321');
    const hit = explorer.locator(
      '[role="treeitem"][data-row-id="enriched[14321].sku"]'
    );
    await expect(hit).toHaveAttribute('aria-selected', 'true');
    // Scrolled into view inside the tree's own scroll area.
    const visibleInTree = await hit.evaluate((row) => {
      const tree = row.closest('[role="tree"]')!.getBoundingClientRect();
      const box = row.getBoundingClientRect();
      return box.top >= tree.top && box.bottom <= tree.bottom;
    });
    expect(visibleInTree).toBe(true);
    expect(Date.now() - started).toBeLessThan(3_000);
    await expect(explorer.getByText('1 of 1')).toBeVisible();
    await expect(
      explorer.getByRole('navigation', { name: 'Selected path' })
    ).toContainText('enriched›[14321]›sku');
  });

  test('declines the text view above its size cap', async ({ page }) => {
    const explorer = page.getByRole('region', { name: 'Run data' });
    await explorer.getByRole('button', { name: 'JSON', exact: true }).click();
    await expect(
      explorer.getByText('This output is too large to show as text')
    ).toBeVisible();
  });

  test('loads an elided input value in full and copies it', async ({
    page,
    context,
  }) => {
    await context.grantPermissions(['clipboard-read', 'clipboard-write']);
    const explorer = page.getByRole('region', { name: 'Run data' });
    await explorer.getByRole('tab', { name: /Input/ }).click();
    await expect(
      explorer.getByText('1 large value not loaded (1.8 MB)')
    ).toBeVisible();

    const upload = explorer.locator(
      '[role="treeitem"][data-row-id="data.upload"]'
    );
    await expect(upload).toContainText('Large value · 1.8 MB');
    await upload.getByRole('button', { name: 'Load full value' }).click();
    await expect(upload).toContainText(
      `${fullUpload.length.toLocaleString('en-US')} chars`
    );

    await upload.click();
    await explorer
      .getByRole('group', { name: 'Selected field' })
      .getByRole('button', { name: 'Copy value' })
      .click();
    await expect
      .poll(() => page.evaluate(() => navigator.clipboard.readText()))
      .toBe(fullUpload);
  });
});

test.describe('Step data explorer (mocked)', () => {
  test('shows a selected step’s large inputs and outputs in the compact explorer', async ({
    page,
    mockApi,
  }) => {
    const workflow = buildWorkflow({ id: 'scn_step', name: 'Step payloads' });
    const instanceId = 'inst_step';
    await mockApi.bootstrap(page);
    await mockApi.workflows.get(page, workflow.id, workflow);
    await mockApi.workflows.instanceDetail(
      page,
      workflow.id,
      instanceId,
      {
        id: instanceId,
        workflowId: workflow.id,
        status: 'COMPLETED',
        usedVersion: 1,
        created: '2026-10-11T09:02:36Z',
        updated: '2026-10-11T09:02:37Z',
        inputs: { data: {}, variables: {} },
        outputs: null,
      },
      { workflowName: workflow.name }
    );
    await mockApi.workflows.stepSummaries(page, workflow.id, instanceId, [
      {
        stepId: 'load',
        stepName: 'Load catalog rows',
        stepType: 'Agent',
        status: 'completed',
        startedAt: '2026-10-11T09:02:37.000Z',
        completedAt: '2026-10-11T09:02:37.102Z',
        durationMs: 102,
        inputs: { limit: 5_000 },
        outputs: {
          instances: Array.from({ length: 5_000 }, (_, i) => ({
            id: `id-${i}`,
            sku: `SKU-${i}`,
          })),
          report: {
            _truncated: true,
            _original_size: 1_258_291,
            _preview: '{"rows":[{"sku":"A-1"',
          },
        },
      },
    ]);
    await mockApi.workflows.stepEvents(page, workflow.id, instanceId, []);
    await new WorkflowHistoryPage(page, workflow.id, instanceId).goto();

    await page.locator('[data-timeline-bar="load"]').click();
    const explorer = page.getByRole('region', {
      name: 'Load catalog rows data',
    });
    await expect(
      explorer.getByRole('tab', { name: /Outputs/ })
    ).toHaveAttribute('aria-selected', 'true');
    await expect(explorer.locator('[data-row-id="report"]')).toContainText(
      'Cut off · 1.2 MB'
    );

    await explorer.getByRole('searchbox').fill('SKU-4321');
    const hit = explorer.locator(
      '[role="treeitem"][data-row-id="instances[4321].sku"]'
    );
    await expect(hit).toHaveAttribute('aria-selected', 'true');
    expect(await explorer.getByRole('treeitem').count()).toBeLessThan(40);
  });
});
