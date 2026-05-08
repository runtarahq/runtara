import { expect } from '@playwright/test';
import { test, buildWorkflow } from '../../../fixtures';

/**
 * Regression guard for the "auto-layout + save mutates step data" bug.
 *
 * Repro: open a workflow whose steps contain a reference with `default`, a composite
 * value with a nested reference that carries a `type` hint, and a Split step with a
 * numeric immediate variable and a composite-array variable. Click **Auto-layout**,
 * then **Save**. The save payload must carry the same MappingValue metadata as the
 * loaded workflow — only `renderingParameters` may differ.
 *
 * Before the fix, the save path dropped `ReferenceValue.default`, the `type` hint on
 * reference/template values inside composites, and coerced numeric Split variables
 * to strings. This test captures the save payload and asserts all of those fields
 * survive.
 */
test.describe('Auto-layout + save preserves step data (mocked)', () => {
  test('does not mutate reference defaults, composite type hints, or Split variables', async ({
    page,
    mockApi,
  }) => {
    const workflowId = 'scn_autolayout_save';
    const workflow = buildWorkflow({
      id: workflowId,
      name: 'Auto-layout fixture',
      currentVersionNumber: 1,
      lastVersionNumber: 1,
      executionGraph: {
        name: 'Auto-layout fixture',
        entryPoint: 'agent',
        steps: {
          agent: {
            id: 'agent',
            stepType: 'Agent',
            agentId: 'http',
            capabilityId: 'http-request',
            inputMapping: {
              // Top-level reference with fallback `default` — this was silently
              // dropped before the fix.
              limit: {
                valueType: 'reference',
                value: 'data.limit',
                type: 'integer',
                default: 10,
              },
              // Composite with a nested reference carrying a `type` hint — the
              // inner `type` was dropped before the fix.
              payload: {
                valueType: 'composite',
                value: {
                  userId: {
                    valueType: 'reference',
                    value: 'steps.api.outputs.user.id',
                    type: 'integer',
                  },
                  name: {
                    valueType: 'immediate',
                    value: 'Alice',
                    type: 'string',
                  },
                },
              },
            },
            renderingParameters: { x: 100, y: 100 },
          },
          splitter: {
            id: 'splitter',
            stepType: 'Split',
            config: {
              value: {
                valueType: 'reference',
                value: 'data.items',
                type: 'json',
              },
              variables: {
                // Numeric immediate — was JSON.stringify-ed on load before the fix.
                counter: {
                  valueType: 'immediate',
                  value: 5,
                  type: 'integer',
                },
                // Composite array — contents were replaced with `{}` before the fix
                // when the outer `type` wasn't exactly `'array'`.
                payload: {
                  valueType: 'composite',
                  value: [
                    { valueType: 'immediate', value: 'a' },
                    { valueType: 'immediate', value: 'b' },
                  ],
                  type: 'array',
                },
              },
            },
            subgraph: {
              entryPoint: 'noop',
              steps: {
                noop: {
                  id: 'noop',
                  stepType: 'Log',
                  message: 'inside split',
                  level: 'info',
                  renderingParameters: { x: 0, y: 0 },
                },
              },
              executionPlan: [],
            },
            renderingParameters: { x: 400, y: 100, width: 320, height: 180 },
          },
        },
        executionPlan: [
          { fromStep: 'agent', toStep: 'splitter', label: 'next' },
        ],
      },
    });

    // Bootstrap the shell and stub the endpoints the editor queries.
    await mockApi.bootstrap(page);
    await mockApi.workflows.get(page, workflowId, workflow);
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
    await page.route(
      new RegExp(`/api/runtime(?:/[^/]+)?/metadata/workflow/step-types`),
      (route) =>
        route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({ step_types: [] }),
        })
    );

    // Capture the save payload. The update endpoint is POST /workflows/{id}/update.
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
          }),
        });
      }
    );

    // Open the workflow editor.
    await page.goto(`/workflows/${workflowId}`);
    await expect(page.locator('main')).toBeVisible();

    // Wait for React Flow to mount a node so we know the editor finished loading.
    await expect(page.locator('.react-flow__node').first()).toBeVisible({
      timeout: 10_000,
    });

    // Click Auto-layout (icon button with title="Auto-layout").
    await page.getByTitle('Auto-layout').click();

    // Auto-layout flips isDirty=true, enabling Save.
    const saveButton = page.getByTitle('Save changes');
    await expect(saveButton).toBeEnabled({ timeout: 5_000 });
    await saveButton.click();

    // Wait for the save request to complete.
    await expect.poll(() => savedPayload, { timeout: 10_000 }).not.toBeNull();

    const savedGraph = savedPayload.executionGraph;
    expect(savedGraph).toBeTruthy();
    expect(savedGraph.steps).toBeTruthy();

    // --- Bug 1: ReferenceValue.default must survive ---
    expect(savedGraph.steps.agent.inputMapping.limit).toEqual({
      valueType: 'reference',
      value: 'data.limit',
      type: 'integer',
      default: 10,
    });

    // --- Bug 2: composite-nested reference type hint must survive ---
    expect(savedGraph.steps.agent.inputMapping.payload.valueType).toBe(
      'composite'
    );
    expect(savedGraph.steps.agent.inputMapping.payload.value.userId).toEqual({
      valueType: 'reference',
      value: 'steps.api.outputs.user.id',
      type: 'integer',
    });
    expect(savedGraph.steps.agent.inputMapping.payload.value.name).toEqual({
      valueType: 'immediate',
      value: 'Alice',
      type: 'string',
    });

    // --- Bug 3: Split variables must not be coerced ---
    expect(savedGraph.steps.splitter.config.value).toMatchObject({
      valueType: 'reference',
      value: 'data.items',
      type: 'json',
    });
    expect(savedGraph.steps.splitter.config.variables.counter).toEqual({
      valueType: 'immediate',
      value: 5, // NUMBER, not string "5"
      type: 'integer',
    });
    expect(savedGraph.steps.splitter.config.variables.payload.valueType).toBe(
      'composite'
    );
    expect(
      Array.isArray(savedGraph.steps.splitter.config.variables.payload.value)
    ).toBe(true);
    expect(savedGraph.steps.splitter.config.variables.payload.value).toEqual([
      { valueType: 'immediate', value: 'a' },
      { valueType: 'immediate', value: 'b' },
    ]);
    expect(savedGraph.steps.splitter.config.variables.payload.type).toBe(
      'array'
    );
  });
});
