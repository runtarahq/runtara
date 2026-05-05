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

type TimelineDropPlacement = 'before' | 'after';

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

  canvasStep(name: string): Locator {
    return this.page.locator(
      `[data-testid="workflow-canvas-node"][data-step-name="${attrValue(name)}"]`
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

  async expectTimelineOrder(stepNames: string[]): Promise<void> {
    await this.useTimelineOnly();
    const actualNames = await this.page
      .getByTestId('timeline-step')
      .evaluateAll((steps, names) => {
        const expected = new Set(names as string[]);
        return steps
          .map((step) => step.getAttribute('data-step-name'))
          .filter(
            (name): name is string => Boolean(name) && expected.has(name)
          );
      }, stepNames);

    expect(actualNames).toEqual(stepNames);
  }

  async expectTimelineChildOrder(
    parentStepName: string,
    childStepNames: string[]
  ): Promise<void> {
    await this.useTimelineOnly();
    const parentNodeId = await this.timelineNodeId(parentStepName);
    const actualNames = await this.page
      .getByTestId('timeline-step')
      .evaluateAll(
        (steps, args) => {
          const { parentId, names } = args as {
            parentId: string;
            names: string[];
          };
          const expected = new Set(names);
          return steps
            .filter(
              (step) => step.getAttribute('data-parent-node-id') === parentId
            )
            .map((step) => step.getAttribute('data-step-name'))
            .filter(
              (name): name is string => Boolean(name) && expected.has(name)
            );
        },
        { parentId: parentNodeId, names: childStepNames }
      );

    expect(actualNames).toEqual(childStepNames);
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

  async addFirstCanvasStep(
    kind: AddStepKind,
    options: StepCreateOptions = {}
  ): Promise<void> {
    await this.beginFirstCanvasStep(kind);
    await this.saveOpenCanvasDialog(options);
  }

  async addCanvasStepAfter(
    sourceStepName: string,
    kind: AddStepKind,
    options: StepCreateOptions = {}
  ): Promise<void> {
    await this.beginCanvasStepAfter(sourceStepName, kind);
    await this.saveOpenCanvasDialog(options);
  }

  async addNestedCanvasStep(
    parentStepName: string,
    kind: AddStepKind,
    options: StepCreateOptions = {}
  ): Promise<void> {
    await this.beginNestedCanvasStep(parentStepName, kind);
    await this.saveOpenCanvasDialog(options);
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

  async beginFirstCanvasStep(kind: AddStepKind): Promise<void> {
    await this.useCanvas();
    await this.page
      .getByRole('button', { name: 'Add first workflow step' })
      .click();
    await this.pickStep(kind);
    await expect(this.page.getByTestId('node-config-dialog')).toBeVisible({
      timeout: 10000,
    });
  }

  async beginCanvasStepAfter(
    sourceStepName: string,
    kind: AddStepKind
  ): Promise<void> {
    await this.useCanvas();
    await this.page
      .getByRole('button', { name: `Add step after ${sourceStepName}` })
      .click();
    await this.pickStep(kind);
    await expect(this.page.getByTestId('node-config-dialog')).toBeVisible({
      timeout: 10000,
    });
  }

  async beginNestedCanvasStep(
    parentStepName: string,
    kind: AddStepKind
  ): Promise<void> {
    await this.useCanvas();
    await this.page
      .getByRole('button', {
        name: `Add first nested step inside ${parentStepName}`,
      })
      .click();
    await this.pickStep(kind);
    await expect(this.page.getByTestId('node-config-dialog')).toBeVisible({
      timeout: 10000,
    });
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

  async configureOpenCanvasSplitSource(source = 'data.items') {
    const dialog = this.page.getByTestId('node-config-dialog');
    await dialog
      .getByPlaceholder("e.g., steps['fetch'].outputs.items")
      .fill(source);
  }

  async configureOpenCanvasCondition(left = 'ready', right = 'ready') {
    const dialog = this.page.getByTestId('node-config-dialog');
    await dialog.getByPlaceholder('Arg 1').fill(left);
    await dialog.getByPlaceholder('Arg 2').fill(right);
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

  async configureOpenEmbedWorkflowVersion(version: number) {
    const panel = this.page.getByTestId('timeline-node-config-panel');
    const versionSelect = panel.getByRole('combobox').nth(1);
    await expect(versionSelect).toBeVisible({ timeout: 15000 });
    await expect(panel.getByText('Loading versions...')).toHaveCount(0, {
      timeout: 15000,
    });
    await versionSelect.click();
    await this.page
      .getByRole('option', { name: new RegExp(`Version ${version}\\b`) })
      .click();
  }

  async expectOpenEmbedWorkflowVersion(version: number) {
    const panel = this.page.getByTestId('timeline-node-config-panel');
    await expect(panel.getByRole('combobox').nth(1)).toContainText(
      `Version ${version}`,
      { timeout: 15000 }
    );
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

  async deleteCanvasStep(stepName: string): Promise<void> {
    await this.useCanvas();
    const node = this.canvasStep(stepName).first();
    await expect(node).toBeVisible({ timeout: 15000 });
    await node.click();
    await this.page.keyboard.press('Delete');
    await expect(this.canvasStep(stepName)).toHaveCount(0);
  }

  async dragTimelineStep(
    sourceStepName: string,
    targetStepName: string,
    placement: TimelineDropPlacement
  ): Promise<void> {
    await this.useTimelineOnly();
    const sourceStep = this.timelineStep(sourceStepName);
    const targetStep = this.timelineStep(targetStepName);
    const handle = sourceStep.getByRole('button', {
      name: `Move ${sourceStepName} with nested steps`,
    });

    await handle.scrollIntoViewIfNeeded();
    await targetStep.scrollIntoViewIfNeeded();

    const handleBox = await handle.boundingBox();
    const targetBox = await targetStep.boundingBox();
    if (!handleBox || !targetBox) {
      throw new Error(
        `Could not locate drag source ${sourceStepName} or target ${targetStepName}`
      );
    }

    const startX = handleBox.x + handleBox.width / 2;
    const startY = handleBox.y + handleBox.height / 2;
    const targetX = targetBox.x + targetBox.width / 2;
    const targetY =
      placement === 'before'
        ? targetBox.y + Math.min(6, targetBox.height / 4)
        : targetBox.y + targetBox.height - Math.min(6, targetBox.height / 4);

    await this.page.mouse.move(startX, startY);
    await this.page.mouse.down();
    await this.page.mouse.move(targetX, targetY, { steps: 10 });
    await this.page.mouse.up();
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

  async expectSaveValidationFailure(): Promise<void> {
    const saveButton = this.page.getByTitle('Save changes');
    await expect(saveButton).toBeEnabled({ timeout: 10000 });

    await saveButton.click();

    await expect(
      this.page.getByRole('button', { name: /Problems/ })
    ).toBeVisible({
      timeout: 15000,
    });
    await this.page.getByRole('button', { name: /Problems/ }).click();
    await expect(
      this.page.getByRole('button', { name: /Errors \([1-9]/ })
    ).toBeVisible({
      timeout: 15000,
    });
    await expect(this.page.getByTitle('Save changes')).toBeVisible({
      timeout: 10000,
    });
  }

  async clearValidationMessages(): Promise<void> {
    await this.page.getByRole('button', { name: /Problems/ }).click();
    const clearButton = this.page.getByTitle('Clear all messages');
    if (await clearButton.isVisible().catch(() => false)) {
      await clearButton.click();
      await expect(this.page.getByText('No problems detected')).toBeVisible({
        timeout: 10000,
      });
    }
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

  async saveOpenCanvasDialog(options: StepCreateOptions = {}): Promise<void> {
    const dialog = this.page.getByTestId('node-config-dialog');
    await expect(dialog).toBeVisible({ timeout: 10000 });

    if (options.name) {
      const nameInput = dialog.getByPlaceholder('Step name');
      await nameInput.clear();
      await nameInput.fill(options.name);
    }

    await dialog.getByRole('button', { name: 'Save' }).click();
    await expect(dialog).toHaveCount(0);
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
