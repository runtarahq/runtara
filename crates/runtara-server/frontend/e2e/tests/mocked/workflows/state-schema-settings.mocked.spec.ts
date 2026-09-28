import { expect } from '@playwright/test';
import { test, buildWorkflow } from '../../../fixtures';
import { appPath } from '../../../utils/app-path';

/**
 * The workflow settings panel edits `stateSchema`: the State section lists
 * the declared fields, a new field is added without the form-only
 * `required` setting, and the save payload carries the whole state schema
 * (labels, the currency format and the enum) next to the other schemas.
 */
test.describe('Workflow state schema settings (mocked)', () => {
  test('adds a state field in settings and saves it with the graph', async ({
    page,
    mockApi,
  }) => {
    const workflowId = 'scn_state_schema_settings';
    const stateSchema = {
      amount: { type: 'number', label: 'Amount', format: 'currency' },
      stage: {
        type: 'string',
        label: 'Stage',
        enum: ['received', 'approval', 'delivered'],
      },
    };
    const workflow = buildWorkflow({
      id: workflowId,
      name: 'State schema fixture',
      currentVersionNumber: 1,
      lastVersionNumber: 1,
      executionGraph: {
        name: 'State schema fixture',
        entryPoint: 'finish',
        steps: {
          finish: {
            id: 'finish',
            stepType: 'Finish',
            renderingParameters: { x: 120, y: 120 },
          },
        },
        executionPlan: [],
        stateSchema,
      },
    });

    await mockApi.bootstrap(page);
    await mockApi.workflows.get(page, workflowId, workflow);
    await mockApi.runtime.metadata(page, { step_types: [] });
    await page.route(
      new RegExp(`/api/runtime(?:/[^/]+)?/workflows/${workflowId}/versions$`),
      (route) =>
        route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            data: [
              {
                version: 1,
                created: '2026-01-01T12:00:00Z',
                trackEvents: false,
              },
            ],
            success: true,
          }),
        })
    );
    await page.route(
      new RegExp(`/api/runtime(?:/[^/]+)?/workflows/${workflowId}/triggers`),
      (route) =>
        route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({ data: [], success: true }),
        })
    );

    let savedPayload: any = null;
    await page.route(
      new RegExp(`/api/runtime(?:/[^/]+)?/workflows/${workflowId}/update`),
      async (route) => {
        savedPayload = route.request().postDataJSON();
        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            data: {
              ...workflow,
              currentVersionNumber: 2,
              lastVersionNumber: 2,
            },
            message: 'ok',
            success: true,
            version: '2',
            warnings: [],
          }),
        });
      }
    );

    // Without an org in the token the app stays on the deep link instead of
    // redirecting to the org-scoped base path.
    await page.addInitScript(() => {
      for (const key of Object.keys(localStorage)) {
        if (!key.startsWith('oidc.user:')) continue;

        const rawValue = localStorage.getItem(key);
        if (!rawValue) continue;

        const user = JSON.parse(rawValue);
        delete user.profile?.org_id;
        localStorage.setItem(key, JSON.stringify(user));
      }
    });

    await page.goto(appPath(`/workflows/${workflowId}`));
    await expect(page.locator('main')).toBeVisible();

    await page.getByRole('button', { name: 'Settings', exact: true }).click();
    await page.getByText('State a run exposes').click();

    const nameInputs = page.getByPlaceholder('fieldName');
    await expect(nameInputs).toHaveCount(2);
    await expect(page.getByPlaceholder('currency')).toHaveValue('currency');
    await expect(
      page.getByRole('columnheader', { name: 'Required' })
    ).toHaveCount(0);

    await page.getByRole('button', { name: 'Add Field' }).click();
    await expect(nameInputs).toHaveCount(3);
    await nameInputs.nth(2).fill('dueAt');

    const saveButton = page.getByTitle('Save changes');
    await expect(saveButton).toBeEnabled({ timeout: 5_000 });
    await saveButton.click();

    await expect.poll(() => savedPayload, { timeout: 10_000 }).not.toBeNull();
    expect(savedPayload.executionGraph.stateSchema).toEqual({
      ...stateSchema,
      dueAt: { type: 'string' },
    });
  });
});
