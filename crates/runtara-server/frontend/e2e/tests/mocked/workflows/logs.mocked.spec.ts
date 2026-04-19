import { test, buildWorkflow } from '../../../fixtures';
import { WorkflowLogsPage } from '../../../pages/WorkflowExtraPages';

test.describe('Workflow execution logs (mocked)', () => {
  test('renders logs view, a11y + snapshot', async ({
    page,
    mockApi,
    runA11y,
  }) => {
    const workflow = buildWorkflow({ id: 'scn_logs', name: 'Logs workflow' });
    const instanceId = 'inst_logs';

    await mockApi.bootstrap(page);
    await mockApi.workflows.get(page, workflow.id, workflow);
    await mockApi.workflows.instance(page, workflow.id, instanceId, {
      data: {
        id: instanceId,
        workflowId: workflow.id,
        status: 'COMPLETED',
        created: '2026-01-01T12:00:00Z',
        finished: '2026-01-01T12:00:10Z',
      },
      success: true,
    });
    await mockApi.workflows.logs(page, workflow.id, instanceId, {
      logs: [
        {
          timestamp: '2026-01-01T12:00:01Z',
          level: 'INFO',
          message: 'Workflow started',
        },
        {
          timestamp: '2026-01-01T12:00:10Z',
          level: 'INFO',
          message: 'Workflow completed',
        },
      ],
    });

    const view = new WorkflowLogsPage(page, workflow.id, instanceId);
    await view.goto();

    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('workflow-logs');
  });
});
