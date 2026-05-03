import { expect, Locator, Page } from '@playwright/test';

type AddStepKind =
  | { type: 'step'; stepType: string }
  | {
      type: 'capability';
      agentId: string;
      capabilityId: string;
      search: string;
    };

type StepCreateOptions = {
  name?: string;
};

function attrValue(value: string): string {
  return value.replace(/\\/g, '\\\\').replace(/"/g, '\\"');
}

export class WorkflowBuilderPage {
  readonly page: Page;

  constructor(page: Page) {
    this.page = page;
  }

  async createWorkflow(name: string): Promise<string> {
    await this.page.goto('/workflows/create');
    await this.page.waitForLoadState('domcontentloaded');
    await this.page.getByLabel('Name').fill(name);
    await this.page.getByRole('button', { name: 'Save' }).click();

    await this.page.waitForURL(
      (url) => /\/workflows\/(?!create\b)[a-zA-Z0-9_-]+$/.test(url.pathname),
      { timeout: 15000 }
    );
    await this.waitForEditor();

    return this.currentWorkflowId();
  }

  async gotoWorkflow(workflowId: string): Promise<void> {
    await this.page.goto(`/workflows/${workflowId}`);
    await this.waitForEditor();
  }

  currentWorkflowId(): string {
    const workflowId = this.page.url().split('/workflows/').pop();
    if (!workflowId || workflowId === 'create') {
      throw new Error(
        `Could not read workflow id from URL: ${this.page.url()}`
      );
    }
    return workflowId;
  }

  async waitForEditor(): Promise<void> {
    await this.page.waitForLoadState('domcontentloaded');
    await expect(
      this.page
        .getByTestId('workflow-timeline')
        .or(this.page.getByTestId('workflow-timeline-empty'))
    ).toBeVisible({ timeout: 15000 });
  }

  async useTimelineOnly(): Promise<void> {
    await this.page.getByTestId('workflow-view-timeline').click();
    await expect(
      this.page
        .getByTestId('workflow-timeline')
        .or(this.page.getByTestId('workflow-timeline-empty'))
    ).toBeVisible();
  }

  async useCanvas(): Promise<void> {
    await this.page.getByTestId('workflow-view-canvas').click();
    await expect(this.page.locator('.react-flow')).toBeVisible({
      timeout: 15000,
    });
  }

  timelineStep(name: string): Locator {
    return this.page.locator(
      `[data-testid="timeline-step"][data-step-name="${attrValue(name)}"]`
    );
  }

  async expectStepVisible(name: string): Promise<void> {
    await expect(this.timelineStep(name)).toBeVisible({ timeout: 15000 });
  }

  async expectStepHidden(name: string): Promise<void> {
    await expect(this.timelineStep(name)).toHaveCount(0);
  }

  async expectStepNestedUnder(
    childStepName: string,
    parentStepName: string
  ): Promise<void> {
    const parentNodeId = await this.timelineNodeId(parentStepName);
    await expect(this.timelineStep(childStepName)).toHaveAttribute(
      'data-parent-node-id',
      parentNodeId
    );
  }

  async timelineNodeId(name: string): Promise<string> {
    await this.expectStepVisible(name);
    const nodeId = await this.timelineStep(name).getAttribute(
      'data-timeline-node-id'
    );
    if (!nodeId) throw new Error(`Timeline step ${name} has no node id`);
    return nodeId;
  }

  async addFirstTimelineStep(
    kind: AddStepKind,
    options: StepCreateOptions = {}
  ): Promise<void> {
    await this.beginFirstTimelineStep(kind);
    await this.saveOpenTimelinePanel(options);
  }

  async addTimelineStepAfter(
    sourceStepName: string,
    kind: AddStepKind,
    options: StepCreateOptions = {}
  ) {
    await this.beginTimelineStepAfter(sourceStepName, kind);
    await this.saveOpenTimelinePanel(options);
  }

  async addNestedTimelineStep(
    parentStepName: string,
    kind: AddStepKind,
    options: StepCreateOptions = {}
  ) {
    await this.beginNestedTimelineStep(parentStepName, kind);
    await this.saveOpenTimelinePanel(options);
  }

  async beginFirstTimelineStep(kind: AddStepKind): Promise<void> {
    await this.useTimelineOnly();
    await this.page.getByTestId('timeline-add-step').first().click();
    await this.pickStep(kind);
    await expect(
      this.page.getByTestId('timeline-node-config-panel')
    ).toBeVisible({ timeout: 10000 });
  }

  async beginTimelineStepAfter(
    sourceStepName: string,
    kind: AddStepKind
  ): Promise<void> {
    await this.useTimelineOnly();
    const sourceNodeId = await this.timelineNodeId(sourceStepName);
    await this.page
      .locator(
        `[data-testid="timeline-add-step"][data-source-node-id="${attrValue(sourceNodeId)}"]`
      )
      .click();
    await this.pickStep(kind);
    await expect(
      this.page.getByTestId('timeline-node-config-panel')
    ).toBeVisible({ timeout: 10000 });
  }

  async beginNestedTimelineStep(
    parentStepName: string,
    kind: AddStepKind
  ): Promise<void> {
    await this.useTimelineOnly();
    const parentNodeId = await this.timelineNodeId(parentStepName);
    await this.page
      .locator(
        `[data-testid="timeline-add-step"][data-parent-node-id="${attrValue(parentNodeId)}"]`
      )
      .first()
      .click();
    await this.pickStep(kind);
    await expect(
      this.page.getByTestId('timeline-node-config-panel')
    ).toBeVisible({ timeout: 10000 });
  }

  async addCanvasBranchStep(
    branchLabel: 'true' | 'false',
    conditionalName: string,
    kind: AddStepKind,
    options: StepCreateOptions = {}
  ): Promise<void> {
    await this.useCanvas();
    await this.page
      .getByRole('button', {
        name: `Add ${branchLabel} branch from ${conditionalName}`,
      })
      .click();
    await this.pickStep(kind);
    const dialog = this.page.getByTestId('node-config-dialog');

    if (options.name) {
      const nameInput = dialog.getByPlaceholder('Step name');
      await nameInput.clear();
      await nameInput.fill(options.name);
    }

    await dialog.getByRole('button', { name: 'Save' }).click();
  }

  async configureSplitSource(stepName: string, source = 'data.items') {
    await this.editTimelineStep(stepName);
    const panel = this.page.getByTestId('timeline-node-config-panel');
    await panel
      .getByPlaceholder("e.g., steps['fetch'].outputs.items")
      .fill(source);
    await panel.getByTestId('timeline-node-config-save').click();
    await expect(panel).toHaveCount(0);
  }

  async configureOpenEmbedWorkflow(childWorkflowName: string) {
    const panel = this.page.getByTestId('timeline-node-config-panel');
    await panel.getByRole('combobox').first().click();
    await this.page.getByRole('option', { name: childWorkflowName }).click();
  }

  async configureOpenCondition(left = 'ready', right = 'ready') {
    const panel = this.page.getByTestId('timeline-node-config-panel');
    await panel.getByPlaceholder('Arg 1').fill(left);
    await panel.getByPlaceholder('Arg 2').fill(right);
  }

  async editTimelineStep(stepName: string): Promise<void> {
    await this.useTimelineOnly();
    await this.timelineStep(stepName)
      .getByRole('button', { name: `Edit ${stepName}` })
      .click();
    await expect(
      this.page.getByTestId('timeline-node-config-panel')
    ).toBeVisible();
  }

  async deleteTimelineStep(stepName: string): Promise<void> {
    await this.editTimelineStep(stepName);
    await this.page.getByTestId('node-form-delete').click();
    await expect(this.timelineStep(stepName)).toHaveCount(0);
  }

  async saveWorkflow(): Promise<void> {
    const saveButton = this.page.getByTitle('Save changes');
    await expect(saveButton).toBeEnabled({ timeout: 10000 });
    const updateResponse = this.page
      .waitForResponse(
        (response) =>
          response.request().method() === 'POST' &&
          /\/api\/runtime\/workflows\/[^/]+\/update$/.test(
            new URL(response.url()).pathname
          ),
        { timeout: 20000 }
      )
      .catch(() => null);

    await saveButton.click();
    const response = await updateResponse;
    if (response && !response.ok()) {
      const body = await response.text();
      throw new Error(
        `Workflow save failed with HTTP ${response.status()}: ${body}`
      );
    }

    await expect(this.page.getByTitle('No changes to save')).toBeVisible({
      timeout: 20000,
    });
  }

  async reloadAndWait(): Promise<void> {
    await this.page.reload();
    await this.waitForEditor();
    await expect(this.page.getByTitle('No changes to save')).toBeVisible({
      timeout: 15000,
    });
  }

  async deleteWorkflowFromList(name: string): Promise<void> {
    await this.page.goto('/workflows');
    await this.page.waitForLoadState('domcontentloaded');
    await this.page.getByPlaceholder('Search workflows...').fill(name);

    const card = this.page.locator('article').filter({ hasText: name });
    await expect(card).toBeVisible({ timeout: 15000 });
    await card.hover();
    await card.getByTitle('Delete').first().click();
    await this.page.getByRole('button', { name: 'Confirm' }).click();
    await expect(card).toHaveCount(0, { timeout: 15000 });
  }

  async saveOpenTimelinePanel(options: StepCreateOptions = {}): Promise<void> {
    const panel = this.page.getByTestId('timeline-node-config-panel');
    await expect(panel).toBeVisible({ timeout: 10000 });

    if (options.name) {
      const nameInput = panel.getByPlaceholder('Step name');
      await nameInput.clear();
      await nameInput.fill(options.name);
    }

    await panel.getByTestId('timeline-node-config-save').click();
    await expect(panel).toHaveCount(0);
  }

  private async pickStep(kind: AddStepKind): Promise<void> {
    if (kind.type === 'step') {
      await this.page
        .getByTestId(`step-picker-step-type-${kind.stepType.toLowerCase()}`)
        .click();
      return;
    }

    await this.page
      .getByPlaceholder('Search steps or operations...')
      .fill(kind.search);
    const result = this.page.getByTestId(
      `step-picker-capability-${kind.agentId}-${kind.capabilityId}`
    );
    await expect(result).toBeVisible({ timeout: 30000 });
    await result.click();
  }
}

export const randomDoubleStep = {
  type: 'capability' as const,
  agentId: 'utils',
  capabilityId: 'random-double',
  search: 'Random Double',
};
