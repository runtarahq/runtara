import { expect } from '@playwright/test';
import { test, buildWorkflow } from '../../../fixtures';
import { InvocationHistoryPage } from '../../../pages/InvocationHistoryPage';

test.describe('Operations Runs (mocked)', () => {
  test('redirects legacy history to Runs with entries, counts, a11y + snapshot', async ({
    page,
    mockApi,
    runA11y,
  }) => {
    const workflow = buildWorkflow({ id: 'scn_h', name: 'History workflow' });

    await mockApi.bootstrap(page);
    await mockApi.workflows.list(page, [workflow]);
    await mockApi.invocationHistory.list(page, [
      {
        id: 'inst_h1',
        workflowId: workflow.id,
        workflowName: workflow.name,
        runLabel: 'ORDER-123',
        status: 'completed',
        created: '2026-01-01T12:00:00Z',
        finished: '2026-01-01T12:00:02Z',
      },
      {
        id: 'inst_h2',
        workflowId: workflow.id,
        workflowName: workflow.name,
        runLabel: 'ORDER-124',
        status: 'failed',
        created: '2026-01-01T12:05:00Z',
      },
    ]);
    await mockApi.raw(
      page,
      /\/api\/runtime(?:\/[^/]+)?\/executions\/summary$/,
      {
        data: { total: 2, counts: { completed: 1, failed: 1 } },
        success: true,
      }
    );

    const view = new InvocationHistoryPage(page);
    await view.goto();

    await expect(page).toHaveURL(/\/operations\/runs$/);
    await view.expectHeading(/^Runs$/);
    await expect(
      page.getByRole('row').filter({
        has: page.getByRole('link', { name: 'ORDER-123', exact: true }),
      })
    ).toContainText('Completed');
    await expect(
      page.getByRole('row').filter({
        has: page.getByRole('link', { name: 'ORDER-124', exact: true }),
      })
    ).toContainText('Failed');
    await expect(
      page.getByRole('button', { name: 'All 2', exact: true })
    ).toBeVisible();
    await expect(page.getByRole('alert')).toHaveCount(0);
    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('operations-runs');
  });
});
