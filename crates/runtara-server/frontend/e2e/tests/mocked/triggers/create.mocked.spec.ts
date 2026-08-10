import { expect, test, buildWorkflow } from '../../../fixtures';
import { CreateTriggerPage } from '../../../pages/TriggersPage';

test.describe('Create trigger (mocked)', () => {
  test('renders form, a11y + snapshot', async ({ page, mockApi, runA11y }) => {
    await mockApi.bootstrap(page);
    await mockApi.workflows.list(page, [
      buildWorkflow({ name: 'Orders sync' }),
    ]);
    await mockApi.connections.list(page, []);
    await mockApi.triggers.list(page, []);

    const view = new CreateTriggerPage(page);
    await view.goto();

    await view.expectHeading(/create trigger/i);

    // Nothing is selected yet, so the workflow picker has to say so rather
    // than render an empty box and only explain itself on submit.
    await expect(
      page.getByRole('combobox').filter({ hasText: 'Select a workflow' })
    ).toBeVisible();

    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('triggers-create');
  });
});
