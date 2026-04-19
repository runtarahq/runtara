import { test, buildWorkflow } from '../../../fixtures';
import { WorkflowChatPage } from '../../../pages/WorkflowExtraPages';

test.describe('Workflow chat without instance (mocked)', () => {
  test('renders chat shell, a11y + snapshot', async ({
    page,
    mockApi,
    runA11y,
  }) => {
    const workflow = buildWorkflow({ id: 'scn_chat', name: 'Chat workflow' });

    await mockApi.bootstrap(page);
    await mockApi.workflows.get(page, workflow.id, workflow);

    const view = new WorkflowChatPage(page, workflow.id);
    await view.goto();

    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('workflow-chat-empty');
  });
});
