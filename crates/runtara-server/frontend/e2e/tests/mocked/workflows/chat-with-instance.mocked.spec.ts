import { test, buildWorkflow } from '../../../fixtures';
import { WorkflowChatPage } from '../../../pages/WorkflowExtraPages';

test.describe('Workflow chat with instance (mocked)', () => {
  test('renders chat with running instance, a11y + snapshot', async ({
    page,
    mockApi,
    runA11y,
  }) => {
    const workflow = buildWorkflow({ id: 'scn_chat2', name: 'Chat workflow' });
    const instanceId = 'inst_chat';

    await mockApi.bootstrap(page);
    await mockApi.workflows.get(page, workflow.id, workflow);
    await mockApi.workflows.instance(page, workflow.id, instanceId, {
      data: {
        id: instanceId,
        workflowId: workflow.id,
        status: 'RUNNING',
        created: '2026-01-01T12:00:00Z',
      },
      success: true,
    });

    const view = new WorkflowChatPage(page, workflow.id, instanceId);
    await view.goto();

    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('workflow-chat-with-instance');
  });
});
