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

    // Querying by accessible name is the assertion: the picker's label has to
    // reach the Radix trigger. Nothing is selected yet, so it also has to say
    // so rather than render an empty box and only explain itself on submit.
    await expect(page.getByRole('combobox', { name: 'Workflow' })).toHaveText(
      /Select a workflow/
    );

    // The selects on this form used to render as unnamed comboboxes. They no
    // longer do, so hold the page to button-name rather than leaving it under
    // the app-wide waiver.
    await runA11y(page, {
      exclude: ['[data-sonner-toaster]'],
      enabledRules: ['button-name'],
    });
    await view.expectMatchesSnapshot('triggers-create');
  });
});
