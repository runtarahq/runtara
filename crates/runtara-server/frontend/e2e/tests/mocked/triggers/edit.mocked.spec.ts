import { test, buildWorkflow, buildTrigger } from '../../../fixtures';
import { EditTriggerPage } from '../../../pages/TriggersPage';

test.describe('Edit trigger (mocked)', () => {
  test('renders populated form, a11y + snapshot', async ({
    page,
    mockApi,
    runA11y,
  }) => {
    const workflow = buildWorkflow({ id: 'scn_1', name: 'Orders sync' });
    const trigger = buildTrigger({
      id: 'trg_edit',
      workflow_id: workflow.id,
      configuration: { type: 'schedule', cron: '0 9 * * *' },
    });

    await mockApi.bootstrap(page);
    await mockApi.workflows.list(page, [workflow]);
    await mockApi.connections.list(page, []);
    await mockApi.triggers.list(page, [trigger]);
    await mockApi.triggers.get(page, trigger.id, trigger);

    const view = new EditTriggerPage(page, trigger.id);
    await view.goto();

    await view.expectHeading(/edit trigger/i);
    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('triggers-edit');
  });
});
