import { expect, type Page } from '@playwright/test';
import { test, buildWorkflow, type MockApi } from '../../../fixtures';
import { WorkflowHistoryPage } from '../../../pages/WorkflowExtraPages';

/**
 * A finished run with nothing to show looks exactly like a run that has not
 * produced anything yet, so the empty states have to read the run's status
 * rather than assume more is coming.
 */
test.describe('Run history empty states (mocked)', () => {
  const workflow = buildWorkflow({
    id: 'scn_empty',
    name: 'Empty run workflow',
  });
  const instanceId = 'inst_empty';

  async function mockRun(page: Page, mockApi: MockApi, status: string) {
    await mockApi.bootstrap(page);
    await mockApi.workflows.get(page, workflow.id, workflow);
    await mockApi.workflows.instance(page, workflow.id, instanceId, {
      data: {
        id: instanceId,
        workflowId: workflow.id,
        status,
        created: '2026-01-01T12:00:00Z',
        finished: status === 'COMPLETED' ? '2026-01-01T12:00:02Z' : undefined,
        inputs: null,
        outputs: null,
      },
      success: true,
    });
    // No steps and no events: the run recorded nothing at all.
    await mockApi.workflows.stepSummaries(page, workflow.id, instanceId, []);
    await mockApi.workflows.stepEvents(page, workflow.id, instanceId, []);
  }

  test('a completed run does not claim results are still coming', async ({
    page,
    mockApi,
  }) => {
    await mockRun(page, mockApi, 'COMPLETED');

    const view = new WorkflowHistoryPage(page, workflow.id, instanceId);
    await view.goto();

    // Output Data card.
    await expect(
      page.getByText('No output data', { exact: true })
    ).toBeVisible();
    await expect(
      page.getByText('This run completed without returning any output')
    ).toBeVisible();

    // Events → Timeline (the default tab).
    await expect(
      page.getByRole('heading', { name: 'No Timeline Events', exact: true })
    ).toBeVisible();
    await expect(
      page.getByText(
        'This run completed without recording any timeline events.'
      )
    ).toBeVisible();

    // Events → List.
    await page.getByRole('tab', { name: 'List' }).click();
    await expect(
      page.getByRole('heading', { name: 'No Events', exact: true })
    ).toBeVisible();
    await expect(
      page.getByText('This run completed without recording any events.')
    ).toBeVisible();

    // Nothing anywhere on the page suggests the run is still going.
    await expect(page.getByText(/still running/i)).toHaveCount(0);
    await expect(page.getByText(/once the workflow completes/i)).toHaveCount(0);
  });

  test('a running run keeps the in-flight wording', async ({
    page,
    mockApi,
  }) => {
    await mockRun(page, mockApi, 'RUNNING');

    const view = new WorkflowHistoryPage(page, workflow.id, instanceId);
    await view.goto();

    await expect(page.getByText('No output data yet')).toBeVisible();
    await expect(
      page.getByText('Output will be available once the workflow completes')
    ).toBeVisible();
    await expect(
      page.getByRole('heading', { name: 'No Timeline Events Yet' })
    ).toBeVisible();
    await expect(page.getByText(/still running/i).first()).toBeVisible();
  });
});
