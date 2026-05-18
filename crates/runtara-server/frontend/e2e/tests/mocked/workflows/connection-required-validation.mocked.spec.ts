import { expect } from '@playwright/test';
import { test, buildWorkflow } from '../../../fixtures';
import { appPath } from '../../../utils/app-path';

test.describe('Connection-required Rust validation (mocked)', () => {
  test('blocks saving an Object Model capability without a connectionId', async ({
    page,
    mockApi,
  }) => {
    const workflowId = 'scn_connection_required_validation';
    const workflow = buildWorkflow({
      id: workflowId,
      name: 'Connection required validation fixture',
      currentVersionNumber: 1,
      lastVersionNumber: 1,
      executionGraph: {
        name: 'Connection required validation fixture',
        entryPoint: 'source',
        steps: {
          source: {
            id: 'source',
            stepType: 'Log',
            name: 'Source Log',
            message: 'source',
            level: 'info',
            renderingParameters: { x: 80, y: 140 },
          },
          query_records: {
            id: 'query_records',
            stepType: 'Agent',
            name: 'Query Records',
            agentId: 'object_model',
            capabilityId: 'query-instances',
            inputMapping: {
              schema_name: {
                valueType: 'immediate',
                value: 'Product',
              },
            },
            renderingParameters: { x: 360, y: 140 },
          },
        },
        executionPlan: [
          { fromStep: 'source', toStep: 'query_records', label: 'next' },
        ],
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

    let updateReached = false;
    await page.route(
      new RegExp(`/api/runtime(?:/[^/]+)?/workflows/${workflowId}/update`),
      (route) => {
        updateReached = true;
        return route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            data: workflow,
            message: 'ok',
            success: true,
            version: '2',
          }),
        });
      }
    );

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
    await expect(
      page.getByRole('button', { name: 'Edit Source Log' })
    ).toBeVisible({
      timeout: 10_000,
    });

    await page.getByTitle('Add note').click();
    const saveButton = page.getByTitle('Save changes');
    await expect(saveButton).toBeEnabled({ timeout: 5_000 });
    await saveButton.click();

    await expect(page.getByRole('button', { name: /Problems/ })).toBeVisible();
    await expect(
      page.getByRole('button', { name: 'Errors (1)' })
    ).toBeVisible();
    await expect(page.getByText(/\[E026\]/).first()).toBeVisible();
    await expect(page.getByText(/requires connection_id/)).toBeVisible();
    await expect(page.getByText('Step: query_records').first()).toBeVisible();
    expect(updateReached).toBe(false);
  });
});
