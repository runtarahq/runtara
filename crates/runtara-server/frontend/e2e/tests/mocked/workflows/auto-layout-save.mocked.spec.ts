import { expect } from '@playwright/test';
import {
  test,
  buildAgentInfo,
  buildCapabilityInfo,
  buildWorkflow,
} from '../../../fixtures';
import { appPath } from '../../../utils/app-path';

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
 *
 * Note on `type`: only `ReferenceValue` carries a `type` hint. `ImmediateValue` and
 * `CompositeValue` are `deny_unknown_fields` with a single `value` field
 * (crates/runtara-dsl/src/schema_types.rs:1559-1563 and :1595-1600), so the editor
 * correctly strips `type` from immediates/composites — asserting it round-trips
 * would be asserting invalid DSL.
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
        // Every `data.*` reference below must be declared, or the Rust
        // validator raises `[E052] … no inputSchema is defined` and the save
        // is blocked before the update request is ever issued.
        inputSchema: {
          limit: { type: 'integer', required: false },
          items: { type: 'array', required: false },
          userId: { type: 'integer', required: false },
        },
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
                    value: 'data.userId',
                    type: 'integer',
                  },
                  // Sibling immediate — must keep its literal untouched while
                  // the reference next to it keeps its `type` hint.
                  name: {
                    valueType: 'immediate',
                    value: 'Alice',
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
                },
                // Composite array — contents were replaced with `{}` before the fix
                // when the outer `type` wasn't exactly `'array'`. A `CompositeValue`
                // has no `type` field at all, so this is the triggering shape.
                payload: {
                  valueType: 'composite',
                  value: [
                    { valueType: 'immediate', value: 'a' },
                    { valueType: 'immediate', value: 'b' },
                  ],
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
    // The save path runs the Rust/WASM validator first and refuses to issue the
    // update request when it reports errors. Without a catalog every Agent step
    // trips `[E020] unknown agent`, so the `http` agent has to be served.
    await mockApi.agents.catalog(page, [
      buildAgentInfo({
        id: 'http',
        name: 'HTTP',
        integrationIds: ['http'],
        capabilities: [
          buildCapabilityInfo({
            id: 'http-request',
            name: 'HTTP Request',
            displayName: 'HTTP Request',
            inputType: 'HttpRequestInput',
            inputs: [
              { name: 'limit', type: 'integer', required: false },
              { name: 'payload', type: 'object', required: false },
            ],
          }),
        ],
      }),
    ]);
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
    await page.goto(appPath(`/workflows/${workflowId}`));
    await expect(page.locator('main')).toBeVisible();

    // The editor boots into the Timeline view and the canvas TabsContent is not
    // force-mounted (src/features/workflows/components/WorkflowEditor/index.tsx),
    // so ReactFlow only mounts once the Canvas tab is selected.
    await page.getByTestId('workflow-view-canvas').click();

    // Wait for React Flow to mount a node so we know the editor finished loading.
    await expect(page.locator('.react-flow__node').first()).toBeVisible({
      timeout: 10_000,
    });

    // Click Auto-layout (icon button with title="Auto-layout"). This rewrites
    // every node position through the store's node<->step round-trip, which is
    // the code path that used to drop MappingValue metadata.
    await page.getByTitle('Auto-layout').click();

    // Auto-layout only flips `isDirty` — the Save button is gated on
    // `isStructurallyDirty` (Workflow/index.tsx passes
    // `isDirty={hasStructuralUnsavedChanges}`), because position-only edits
    // must not block execution. Add a note to make the graph structurally
    // dirty so Save becomes clickable and the layouted graph gets serialized.
    await page.getByTitle('Add note').click();

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
      value: 'data.userId',
      type: 'integer',
    });
    expect(savedGraph.steps.agent.inputMapping.payload.value.name).toEqual({
      valueType: 'immediate',
      value: 'Alice',
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
    });
    expect(
      typeof savedGraph.steps.splitter.config.variables.counter.value
    ).toBe('number');
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
  });
});
