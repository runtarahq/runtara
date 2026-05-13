import { expect } from '@playwright/test';
import { test, buildWorkflow } from '../../../fixtures';

test.describe('Step edit Rust validation (mocked)', () => {
  test('blocks applying an invalid Error step edit before it reaches editor state', async ({
    page,
    mockApi,
  }) => {
    const workflowId = 'scn_step_edit_rust_validation';
    const workflow = buildWorkflow({
      id: workflowId,
      name: 'Step edit Rust validation fixture',
      currentVersionNumber: 1,
      lastVersionNumber: 1,
      executionGraph: {
        name: 'Step edit Rust validation fixture',
        entryPoint: 'error',
        steps: {
          error: {
            id: 'error',
            stepType: 'Error',
            name: 'Validation Target',
            code: 'ORIGINAL_CODE',
            message: 'Original message',
            category: 'permanent',
            severity: 'error',
            renderingParameters: { x: 120, y: 120 },
          },
        },
        executionPlan: [],
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

    await page.goto(`/workflows/${workflowId}`);
    await expect(page.locator('main')).toBeVisible();

    await page.getByRole('button', { name: 'Edit Validation Target' }).click();

    const panel = page.getByTestId('timeline-node-config-panel');
    await expect(panel).toBeVisible({ timeout: 5_000 });
    await expect(panel.getByText('Error Code *')).toBeVisible();

    await panel.getByPlaceholder('Enter error code...').clear();
    await panel.getByPlaceholder('Enter error message...').clear();
    await panel.getByTestId('timeline-node-config-save').click();

    await expect(panel).toBeVisible();
    await expect(page.getByText(/Failed to parse graph/)).toBeVisible({
      timeout: 10_000,
    });
    await expect(page.getByText('Step: Validation Target')).toBeVisible();
  });
});
