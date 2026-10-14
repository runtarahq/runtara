import { expect, type Page } from '@playwright/test';
import { test, buildWorkflow } from '../../../fixtures';
import { appPath } from '../../../utils/app-path';

/**
 * Step ids are unique per graph, not per workflow: two Splits may each hold
 * an `iter_finish`. React Flow node ids are global, so the canvas used to
 * collapse the two into one node — drawing the first Split's inner edge into
 * the second container — and a save then rewrote the workflow: the first
 * Finish took the second one's data and its edge moved to the root plan.
 */
test.describe('Step ids shared across subgraphs (mocked)', () => {
  const split = (
    id: string,
    body: string,
    finishName: string,
    wired = true
  ) => ({
    id,
    name: `Split ${id}`,
    stepType: 'Split',
    config: { value: { valueType: 'reference', value: 'data.rows' } },
    inputSchema: {},
    subgraph: {
      entryPoint: body,
      steps: {
        [body]: {
          id: body,
          name: `Log ${id}`,
          stepType: 'Log',
          message: `inside ${id}`,
          level: 'info',
        },
        iter_finish: {
          id: 'iter_finish',
          name: finishName,
          stepType: 'Finish',
          inputMapping: {},
        },
      },
      executionPlan: wired
        ? [{ fromStep: body, toStep: 'iter_finish', label: 'next' }]
        : [],
    },
  });

  async function boxOf(page: Page, selector: string) {
    const box = await page.locator(selector).boundingBox();
    expect(box, selector).not.toBeNull();
    return box!;
  }

  const workflowId = 'scn_shared_step_ids';
  const json = (body: unknown) => ({
    status: 200,
    contentType: 'application/json',
    body: JSON.stringify(body),
  });

  /** Stub the editor's endpoints for the two-Split workflow. */
  async function mockSharedIdWorkflow(
    page: Page,
    mockApi: any,
    { wireB = true, finishNameB = 'Notification complete' } = {}
  ) {
    const workflow = buildWorkflow({
      id: workflowId,
      name: 'Shared step ids',
      currentVersionNumber: 1,
      lastVersionNumber: 1,
      executionGraph: {
        name: 'Shared step ids',
        entryPoint: 'a',
        inputSchema: { rows: { type: 'array', required: false } },
        steps: {
          a: split('a', 'log_a', 'Post analyzed'),
          b: split('b', 'log_b', finishNameB, wireB),
          finish: {
            id: 'finish',
            name: 'Done',
            stepType: 'Finish',
            inputMapping: {},
          },
        },
        executionPlan: [
          { fromStep: 'a', toStep: 'b', label: 'next' },
          { fromStep: 'b', toStep: 'finish', label: 'next' },
        ],
      },
    });

    await mockApi.bootstrap(page);
    await mockApi.agents.catalog(page, []);
    await mockApi.workflows.get(page, workflowId, workflow);
    await page.route(
      new RegExp(`/api/runtime(?:/[^/]+)?/workflows/${workflowId}/versions$`),
      (route) =>
        route.fulfill(
          json({
            data: [{ version: 1, created: '2026-01-01T12:00:00Z' }],
            success: true,
          })
        )
    );
    await page.route(
      new RegExp(`/api/runtime(?:/[^/]+)?/workflows/${workflowId}/triggers`),
      (route) => route.fulfill(json({ data: [], success: true }))
    );
    await page.route(
      new RegExp(`/api/runtime(?:/[^/]+)?/metadata/workflow/step-types`),
      (route) => route.fulfill(json({ step_types: [] }))
    );
    return workflow;
  }

  test('renders each Split with its own Finish and saves them unchanged', async ({
    page,
    mockApi,
  }) => {
    const workflow = await mockSharedIdWorkflow(page, mockApi);

    let savedPayload: any = null;
    await page.route(
      new RegExp(`/api/runtime(?:/[^/]+)?/workflows/${workflowId}/update`),
      async (route) => {
        savedPayload = route.request().postDataJSON();
        await route.fulfill(
          json({
            data: {
              ...workflow,
              currentVersionNumber: 2,
              lastVersionNumber: 2,
            },
            message: 'ok',
            success: true,
            version: '2',
          })
        );
      }
    );

    await page.goto(appPath(`/workflows/${workflowId}`));
    await expect(page.locator('.react-flow__node').first()).toBeVisible({
      timeout: 10_000,
    });

    // Both Finish steps are on the canvas, each inside its own Split.
    const finishes = page.locator('.react-flow__node', {
      hasText: /Post analyzed|Notification complete/,
    });
    await expect(finishes).toHaveCount(2);
    for (const [container, finishName] of [
      ['a', 'Post analyzed'],
      ['b', 'Notification complete'],
    ]) {
      const containerBox = await boxOf(
        page,
        `.react-flow__node[data-id="${container}"]`
      );
      const finishBox = (await page
        .locator('.react-flow__node', { hasText: finishName })
        .boundingBox())!;
      expect(finishBox.x).toBeGreaterThanOrEqual(containerBox.x);
      expect(finishBox.x + finishBox.width).toBeLessThanOrEqual(
        containerBox.x + containerBox.width
      );
      expect(finishBox.y).toBeGreaterThanOrEqual(containerBox.y);
      expect(finishBox.y + finishBox.height).toBeLessThanOrEqual(
        containerBox.y + containerBox.height
      );

      // The Split's inner edge stays inside its container.
      const edge = page.locator(
        `.react-flow__edge[data-id^="log_${container}-"] path`
      );
      const edgeBox = (await edge.first().boundingBox())!;
      expect(edgeBox.x).toBeGreaterThanOrEqual(containerBox.x);
      expect(edgeBox.x + edgeBox.width).toBeLessThanOrEqual(
        containerBox.x + containerBox.width
      );
    }

    await page.getByTitle('Add note').click();
    const saveButton = page.getByTitle('Save changes');
    await expect(saveButton).toBeEnabled({ timeout: 5_000 });
    await saveButton.click();
    await expect.poll(() => savedPayload, { timeout: 10_000 }).not.toBeNull();

    const saved = savedPayload.executionGraph;
    const planOf = (graph: any) =>
      (graph.executionPlan ?? []).map((e: any) => `${e.fromStep}->${e.toStep}`);
    expect(planOf(saved)).toEqual(['a->b', 'b->finish']);
    expect(Object.keys(saved.steps).sort()).toEqual(['a', 'b', 'finish']);
    for (const [container, finishName] of [
      ['a', 'Post analyzed'],
      ['b', 'Notification complete'],
    ]) {
      const sub = saved.steps[container].subgraph;
      expect(Object.keys(sub.steps).sort()).toEqual([
        'iter_finish',
        `log_${container}`,
      ]);
      expect(sub.steps.iter_finish.id).toBe('iter_finish');
      expect(sub.steps.iter_finish.name).toBe(finishName);
      expect(planOf(sub)).toEqual([`log_${container}->iter_finish`]);
    }
  });
  test('shows each run status on the step that ran, by iteration scope', async ({
    page,
    mockApi,
  }) => {
    await mockSharedIdWorkflow(page, mockApi);
    const instanceId = 'inst_shared_step_ids';
    await mockApi.workflows.instanceDetail(page, workflowId, instanceId, {
      id: instanceId,
      workflowId,
      status: 'failed',
      steps: [],
    });
    // Both Splits' `iter_finish` fail, each with its own error; only the
    // event's scope tells the two apart.
    let eventId = 0;
    const stepEvents = (stepId: string, scopeId: string, error: string) => [
      {
        id: ++eventId,
        eventType: 'custom',
        subtype: 'step_debug_start',
        payload: { step_id: stepId, scope_id: scopeId, timestamp_ms: 1 },
      },
      {
        id: ++eventId,
        eventType: 'custom',
        subtype: 'step_debug_end',
        payload: {
          step_id: stepId,
          scope_id: scopeId,
          timestamp_ms: 2,
          duration_ms: 1,
          error,
        },
      },
    ];
    await mockApi.workflows.stepEvents(page, workflowId, instanceId, [
      ...stepEvents('iter_finish', 'sc_a_0', 'failed inside a'),
      ...stepEvents('iter_finish', 'sc_b_0', 'failed inside b'),
    ]);

    await page.goto(
      appPath(`/workflows/${workflowId}?attachInstance=${instanceId}`)
    );
    const finishA = page.locator('.react-flow__node', {
      hasText: 'Post analyzed',
    });
    const finishB = page.locator('.react-flow__node', {
      hasText: 'Notification complete',
    });
    await expect(finishA).toContainText('failed inside a', { timeout: 10_000 });
    await expect(finishB).toContainText('failed inside b');
    await expect(finishA).not.toContainText('failed inside b');
  });

  test('step-id field shows the DSL step id and flags a shared one', async ({
    page,
    mockApi,
  }) => {
    await mockSharedIdWorkflow(page, mockApi);
    await page.goto(appPath(`/workflows/${workflowId}`));

    // Split b's Finish is the one with a qualified canvas id.
    await page
      .locator('.react-flow__node', { hasText: 'Notification complete' })
      .dblclick();
    const editButton = page.getByRole('button', { name: 'Edit step id' });
    await expect(editButton).toBeVisible({ timeout: 10_000 });
    const idRow = editButton.locator('..');
    await expect(idRow).toHaveText('iter_finish');
    await expect(
      page.getByText('Another container also has a step with this id')
    ).toBeVisible();
  });
  test('flags a browser-validation error on the body that has it', async ({
    page,
    mockApi,
  }) => {
    // Only Split b's `iter_finish` is unwired (E003).
    await mockSharedIdWorkflow(page, mockApi, { wireB: false });
    await page.goto(appPath(`/workflows/${workflowId}`));
    const finishA = page.locator('.react-flow__node', {
      hasText: 'Post analyzed',
    });
    const finishB = page.locator('.react-flow__node', {
      hasText: 'Notification complete',
    });
    await expect(finishB).toBeVisible({ timeout: 10_000 });

    await page.getByTitle('Add note').click();
    const saveButton = page.getByTitle('Save changes');
    await expect(saveButton).toBeEnabled({ timeout: 5_000 });
    await saveButton.click();

    await expect(finishB).toContainText('not reachable', { timeout: 10_000 });
    await expect(finishA).not.toContainText('not reachable');
  });

  test('flags a server-validation error on the body it names', async ({
    page,
    mockApi,
  }) => {
    await mockSharedIdWorkflow(page, mockApi);
    // The graph is valid in the browser; the server rejects the save naming
    // Split b's `iter_finish`.
    await page.route(
      new RegExp(`/api/runtime(?:/[^/]+)?/workflows/${workflowId}/update`),
      (route) =>
        route.fulfill({
          status: 400,
          contentType: 'application/json',
          body: JSON.stringify({
            success: false,
            message: 'Workflow validation failed',
            validationErrors: [
              {
                code: 'E999',
                message: 'server says b is wrong',
                stepId: 'iter_finish',
                graphPath: ['b'],
              },
            ],
          }),
        })
    );
    await page.goto(appPath(`/workflows/${workflowId}`));
    const finishA = page.locator('.react-flow__node', {
      hasText: 'Post analyzed',
    });
    const finishB = page.locator('.react-flow__node', {
      hasText: 'Notification complete',
    });
    await expect(finishB).toBeVisible({ timeout: 10_000 });

    await page.getByTitle('Add note').click();
    const saveButton = page.getByTitle('Save changes');
    await expect(saveButton).toBeEnabled({ timeout: 5_000 });
    await saveButton.click();

    await expect(finishB).toContainText('server says b is wrong', {
      timeout: 10_000,
    });
    await expect(finishA).not.toContainText('server says b is wrong');
  });
  test('flags a duplicate step name on each step that has it', async ({
    page,
    mockApi,
  }) => {
    // The v28 shape: both Splits' `iter_finish` are named alike (E060).
    await mockSharedIdWorkflow(page, mockApi, { finishNameB: 'Post analyzed' });
    await page.goto(appPath(`/workflows/${workflowId}`));
    const finishA = page.locator('.react-flow__node[data-id="iter_finish"]');
    const finishB = page.locator('.react-flow__node[data-id="b/iter_finish"]');
    await expect(finishB).toBeVisible({ timeout: 10_000 });

    await page.getByTitle('Add note').click();
    const saveButton = page.getByTitle('Save changes');
    await expect(saveButton).toBeEnabled({ timeout: 5_000 });
    await saveButton.click();

    await expect(finishA).toContainText('same name', { timeout: 10_000 });
    await expect(finishB).toContainText('same name');
  });
});
