import { expect } from '@playwright/test';
import { test, buildWorkflow } from '../../../fixtures';

test.describe('Template reference Rust validation (mocked)', () => {
  test('shows static Minijinja reference warnings from WASM without blocking save', async ({
    page,
    mockApi,
  }) => {
    const workflowId = 'scn_template_reference_validation';
    const workflow = buildWorkflow({
      id: workflowId,
      name: 'Template reference validation fixture',
      currentVersionNumber: 1,
      lastVersionNumber: 1,
      executionGraph: {
        name: 'Template reference validation fixture',
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
          finish: {
            id: 'finish',
            stepType: 'Finish',
            name: 'Summary Output',
            inputMapping: {
              summary: {
                valueType: 'template',
                value: 'Archive: {{ steps.missing_archive.outputs.file }}',
              },
            },
            renderingParameters: { x: 160, y: 140 },
          },
        },
        executionPlan: [{ fromStep: 'source', toStep: 'finish' }],
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

    let savedPayload: any = null;
    let releaseUpdate: (() => void) | undefined;
    const updateCanComplete = new Promise<void>((resolve) => {
      releaseUpdate = resolve;
    });

    await page.route(
      new RegExp(`/api/runtime(?:/[^/]+)?/workflows/${workflowId}/update`),
      async (route) => {
        savedPayload = route.request().postDataJSON();
        await updateCanComplete;
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
          }),
        });
      }
    );

    await page.goto(`/workflows/${workflowId}`);
    await expect(page.locator('main')).toBeVisible();
    await expect(
      page.getByRole('button', { name: /Summary Output Finish/ })
    ).toBeVisible({
      timeout: 10_000,
    });

    await page.getByRole('button', { name: 'Edit Source Log' }).click();
    const panel = page.getByTestId('timeline-node-config-panel');
    await expect(panel).toBeVisible({ timeout: 5_000 });
    await panel.getByPlaceholder('Enter log message...').fill('source updated');
    await panel.getByTestId('timeline-node-config-save').click();
    await expect(panel).toBeHidden({ timeout: 10_000 });

    const saveButton = page.getByTitle('Save changes');
    await expect(saveButton).toBeEnabled({ timeout: 5_000 });

    try {
      await saveButton.click();

      await expect.poll(() => savedPayload, { timeout: 10_000 }).not.toBeNull();

      expect(
        savedPayload.executionGraph.steps.finish.inputMapping.summary
      ).toEqual({
        valueType: 'template',
        value: 'Archive: {{ steps.missing_archive.outputs.file }}',
      });

      await page.getByRole('button', { name: /Problems/ }).click();

      await expect(
        page.getByRole('button', { name: 'Warnings (1)' })
      ).toBeVisible();
      await expect(
        page.getByRole('button', { name: 'Errors (0)' })
      ).toBeVisible();
      await page.getByRole('button', { name: 'Warnings (1)' }).click();

      await expect(page.getByText(/\[W052\]/).first()).toBeVisible();
      await expect(page.getByText(/\[W000\]/)).toHaveCount(0);
      await expect(page.getByText(/missing_archive/)).toBeVisible();
      await expect(page.getByText(/does not exist/)).toBeVisible();
      await expect(page.getByText('Step: finish')).toBeVisible();
    } finally {
      releaseUpdate?.();
    }
  });
});
