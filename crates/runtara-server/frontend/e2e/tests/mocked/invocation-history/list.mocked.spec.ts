import { test, buildWorkflow } from '../../../fixtures';
import { InvocationHistoryPage } from '../../../pages/InvocationHistoryPage';

test.describe('Invocation history (mocked)', () => {
  test('renders history with entries, a11y + snapshot', async ({
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
        status: 'COMPLETED',
        created: '2026-01-01T12:00:00Z',
        finished: '2026-01-01T12:00:02Z',
      },
      {
        id: 'inst_h2',
        workflowId: workflow.id,
        status: 'FAILED',
        created: '2026-01-01T12:05:00Z',
      },
    ]);

    const view = new InvocationHistoryPage(page);
    await view.goto();

    await view.expectHeading(/invocation history/i);
    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('invocation-history');
  });
});
