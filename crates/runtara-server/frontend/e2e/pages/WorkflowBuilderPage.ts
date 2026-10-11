import { expect, Locator, Page } from '@playwright/test';
import {
  appPath,
  appPathExactPattern,
  appPathPattern,
  appRoutePattern,
} from '../utils/app-path';

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

type RouteHandle = 'true' | 'false' | 'default' | 'onError' | `case-${number}`;

function attrValue(value: string): string {
  return value.replace(/\\/g, '\\\\').replace(/"/g, '\\"');
}

export class WorkflowBuilderPage {
  readonly page: Page;

  constructor(page: Page) {
    this.page = page;
  }

  async createWorkflow(name: string): Promise<string> {
    await this.page.goto(appPath('/workflows/create'));
    await this.page.waitForLoadState('domcontentloaded');
    await this.page.getByLabel('Name').fill(name);
    await this.page.getByRole('button', { name: 'Save' }).click();

    await this.page.waitForURL(
      (url) =>
        appRoutePattern('/workflows/(?!create\\b)[a-zA-Z0-9_-]+$').test(
          url.pathname
        ),
      { timeout: 15000 }
    );
    await this.waitForEditor();

    return this.currentWorkflowId();
  }

  async gotoWorkflow(workflowId: string): Promise<void> {
    await this.page.goto(appPath(`/workflows/${workflowId}`));
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
    await expect(this.page.locator('.react-flow')).toBeVisible({
      timeout: 15000,
    });
  }

  /**
   * Fits the whole workflow into view. The canvas opens at a readable zoom
   * anchored on the start, so steps far down a long workflow can sit outside
   * the pane, where they cannot be clicked.
   */
  async useCanvas(): Promise<void> {
    await this.waitForEditor();
    await this.page.locator('.react-flow__controls-fitview').click();
  }

  canvasStep(name: string): Locator {
    return this.page.locator(
      `[data-testid="workflow-canvas-node"][data-step-name="${attrValue(name)}"]`
    );
  }

  async expectStepVisible(name: string): Promise<void> {
    await expect(this.canvasStep(name).first()).toBeVisible({
      timeout: 15000,
    });
  }

  async expectStepHidden(name: string): Promise<void> {
    await expect(this.canvasStep(name)).toHaveCount(0);
  }

  async expectStepNestedUnder(
    childStepName: string,
    parentStepName: string
  ): Promise<void> {
    const parentNodeId = await this.canvasNodeId(parentStepName);
    await expect(this.canvasStep(childStepName).first()).toHaveAttribute(
      'data-parent-node-id',
      parentNodeId
    );
  }

  /** The canvas draws an edge from the source step's handle to the child. */
  async expectStepOnRoute(
    childStepName: string,
    sourceStepName: string,
    sourceHandle: RouteHandle
  ): Promise<void> {
    const sourceNodeId = await this.canvasNodeId(sourceStepName);
    const childNodeId = await this.canvasNodeId(childStepName);
    await expect(
      this.page.locator(
        `[data-edge-source="${attrValue(sourceNodeId)}"][data-edge-target="${attrValue(childNodeId)}"][data-edge-handle="${attrValue(sourceHandle)}"]`
      )
    ).toHaveCount(1);
  }

  async canvasNodeId(name: string): Promise<string> {
    const step = this.canvasStep(name).first();
    await expect(step).toBeAttached({ timeout: 15000 });
    const nodeId = await step.evaluate((element) =>
      element.closest('.react-flow__node')?.getAttribute('data-id')
    );
    if (!nodeId) throw new Error(`Canvas step ${name} has no node id`);
    return nodeId;
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

  async addCanvasRouteStep(
    sourceStepName: string,
    sourceHandle: RouteHandle,
    kind: AddStepKind,
    options: StepCreateOptions = {}
  ) {
    await this.beginCanvasRouteStep(sourceStepName, sourceHandle, kind);
    await this.saveOpenCanvasDialog(options);
  }

  /** The error-handler button shows once the step is selected. */
  async addCanvasErrorHandler(
    sourceStepName: string,
    options: StepCreateOptions = {}
  ) {
    await this.selectCanvasStep(sourceStepName);
    await this.page
      .getByRole('button', {
        name: `Add error handler from ${sourceStepName}`,
      })
      .click();
    await expect(this.page.getByTestId('node-config-dialog')).toBeVisible({
      timeout: 10000,
    });
    await this.saveOpenCanvasDialog(options);
  }

  async selectCanvasStep(name: string): Promise<void> {
    await this.useCanvas();
    await this.canvasStep(name).first().click();
  }

  async beginCanvasRouteStep(
    sourceStepName: string,
    sourceHandle: RouteHandle,
    kind: AddStepKind
  ): Promise<void> {
    await this.useCanvas();
    await this.page
      .getByRole('button', {
        name: this.routeButtonName(sourceStepName, sourceHandle),
      })
      .click();
    await this.pickStep(kind);
    await expect(this.page.getByTestId('node-config-dialog')).toBeVisible({
      timeout: 10000,
    });
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

    await dialog.getByTestId('node-config-save').click();
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
    await this.editCanvasStep(stepName);
    const panel = this.page.getByTestId('node-config-dialog');
    await panel
      .getByPlaceholder("e.g., steps['fetch'].outputs.items")
      .fill(source);
    await panel.getByTestId('node-config-save').click();
    await expect(panel).toHaveCount(0);
  }

  async configureOpenEmbedWorkflow(childWorkflowName: string) {
    const panel = this.page.getByTestId('node-config-dialog');
    await panel.getByRole('combobox').first().click();
    await this.page.getByRole('option', { name: childWorkflowName }).click();
  }

  async configureOpenEmbedWorkflowVersion(version: number) {
    const panel = this.page.getByTestId('node-config-dialog');
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
    const panel = this.page.getByTestId('node-config-dialog');
    await expect(panel.getByRole('combobox').nth(1)).toContainText(
      `Version ${version}`,
      { timeout: 15000 }
    );
  }

  async configureOpenCondition(left = 'ready', right = 'ready') {
    const panel = this.page.getByTestId('node-config-dialog');
    await panel.getByPlaceholder('Arg 1').fill(left);
    await panel.getByPlaceholder('Arg 2').fill(right);
  }

  async configureOpenSwitchRouting(
    switchValue = 'status',
    firstCaseMatch = 'approved'
  ) {
    const panel = this.page.getByTestId('node-config-dialog');
    await panel
      .getByPlaceholder('Enter value or use reference mode...')
      .fill(switchValue);
    await panel.getByTestId('switch-routing-mode').click();
    await panel.getByTestId('switch-add-case').click();
    await panel.getByTestId('switch-case-match-0').fill(firstCaseMatch);
    await panel.getByTestId('switch-case-match-0').blur();
  }

  /** Opens the step's editor (double-click) — the only way into it on the canvas. */
  async editCanvasStep(stepName: string): Promise<void> {
    await this.useCanvas();
    await this.canvasStep(stepName).first().dblclick();
    await expect(this.page.getByTestId('node-config-dialog')).toBeVisible();
  }

  async deleteCanvasStep(stepName: string): Promise<void> {
    await this.useCanvas();
    const node = this.canvasStep(stepName).first();
    await expect(node).toBeVisible({ timeout: 15000 });
    await node.click();
    await this.page.keyboard.press('Delete');
    await expect(this.canvasStep(stepName)).toHaveCount(0);
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
    await this.page.goto(appPath('/workflows'));
    await this.page.waitForLoadState('domcontentloaded');
    await this.page.getByPlaceholder('Search workflows...').fill(name);

    const card = this.page.locator('article').filter({ hasText: name });
    await expect(card).toBeVisible({ timeout: 15000 });
    await card.hover();
    await card.getByTitle('Delete').first().click();
    await this.page.getByRole('button', { name: 'Confirm' }).click();
    await expect(card).toHaveCount(0, { timeout: 15000 });
  }

  async createOpenAiConnection(name: string): Promise<void> {
    await this.page.goto(appPath('/connections'));
    await this.page.waitForLoadState('domcontentloaded');
    await this.page.getByRole('button', { name: /new connection/i }).click();
    const dialog = this.page.getByRole('dialog');
    await expect(dialog).toBeVisible({ timeout: 15000 });
    await dialog.getByText('OpenAI', { exact: true }).click();
    await expect(this.page).toHaveURL(
      appPathPattern('/connections/openai_api_key/create')
    );
    await this.page.waitForLoadState('domcontentloaded');

    await this.page.getByLabel('Title').fill(name);
    await this.page
      .getByPlaceholder('sk-xxxxxxxxxxxxx')
      .fill('sk-test-workflow-builder');
    const baseUrl = this.page.getByLabel('Base URL');
    if (await baseUrl.isVisible().catch(() => false)) {
      await baseUrl.fill('https://api.openai.com/v1');
    }

    await this.page.getByRole('button', { name: 'Create connection' }).click();
    await expect(this.page).toHaveURL(appPathExactPattern('/connections'), {
      timeout: 15000,
    });
    await expect(this.page.getByText(name)).toBeVisible({ timeout: 15000 });
  }

  async deleteConnectionFromList(name: string): Promise<void> {
    await this.page.goto(appPath('/connections'));
    await this.page.waitForLoadState('domcontentloaded');
    const card = this.page.locator('article').filter({ hasText: name });
    await expect(card).toBeVisible({ timeout: 15000 });
    await card.getByTitle('Delete connection').first().click();
    await this.page.getByRole('button', { name: /delete connection/i }).click();
    await expect(card).toHaveCount(0, { timeout: 15000 });
  }

  async configureOpenAiAgent(connectionName: string): Promise<void> {
    const panel = this.page.getByTestId('node-config-dialog');
    await panel.getByRole('combobox').first().click();
    await this.page.getByRole('option', { name: 'OpenAI' }).click();

    const connectionSelect = panel.getByRole('combobox').nth(1);
    await expect(connectionSelect).toBeEnabled({ timeout: 15000 });
    await connectionSelect.click();
    await this.page.getByRole('option', { name: connectionName }).click();

    await panel
      .getByPlaceholder('You are a helpful assistant...')
      .fill('Use the configured tools when helpful.');
    await panel
      .getByPlaceholder('Enter user prompt or select a reference...')
      .fill('Summarize the workflow state.');

    const modelSelect = panel.getByRole('combobox').nth(2);
    await modelSelect.click();
    await this.page.getByRole('option', { name: /GPT-4o Mini/ }).click();
  }

  async addCanvasAiTool(
    sourceStepName: string,
    kind: AddStepKind,
    options: StepCreateOptions = {}
  ): Promise<void> {
    await this.useCanvas();
    await this.page
      .getByRole('button', { name: `Add tool to ${sourceStepName}` })
      .click();
    await this.pickStep(kind);
    await expect(this.page.getByTestId('node-config-dialog')).toBeVisible({
      timeout: 10000,
    });
    await this.saveOpenCanvasDialog(options);
  }

  async addCanvasAiMemory(sourceStepName: string): Promise<void> {
    await this.useCanvas();
    await this.page
      .getByRole('button', { name: `Add memory to ${sourceStepName}` })
      .click();
    const objectModelAgent = this.page.getByTestId(
      'step-picker-agent-object-model'
    );
    await expect(objectModelAgent).toBeVisible({ timeout: 30000 });
    await objectModelAgent.click();
    await expect(
      this.page.getByRole('button', { name: `Add memory to ${sourceStepName}` })
    ).toHaveCount(0, { timeout: 10000 });
  }

  async expectOpenAiAgentToolAndMemory(
    stepName: string,
    toolName = 'random_double'
  ): Promise<void> {
    await this.editCanvasStep(stepName);
    const panel = this.page.getByTestId('node-config-dialog');
    await expect(panel.getByText(toolName, { exact: true })).toBeVisible({
      timeout: 15000,
    });
    await expect(panel.getByText('Conversation Memory')).toBeVisible({
      timeout: 15000,
    });
  }

  async saveOpenCanvasDialog(options: StepCreateOptions = {}): Promise<void> {
    const dialog = this.page.getByTestId('node-config-dialog');
    await expect(dialog).toBeVisible({ timeout: 10000 });

    if (options.name) {
      const nameInput = dialog.getByPlaceholder('Step name');
      await nameInput.clear();
      await nameInput.fill(options.name);
    }

    await dialog.getByTestId('node-config-save').click();
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

  private routeButtonName(
    sourceStepName: string,
    sourceHandle: RouteHandle
  ): string {
    if (sourceHandle === 'true' || sourceHandle === 'false') {
      return `Add ${sourceHandle} branch from ${sourceStepName}`;
    }
    if (sourceHandle === 'default') {
      return `Add default route from ${sourceStepName}`;
    }
    if (sourceHandle === 'onError') {
      return `Add error handler from ${sourceStepName}`;
    }

    const caseMatch = /^case-(\d+)$/.exec(sourceHandle);
    if (caseMatch) {
      return `Add case ${Number(caseMatch[1]) + 1} route from ${sourceStepName}`;
    }

    return `Add ${sourceHandle} route from ${sourceStepName}`;
  }
}

export const randomDoubleStep = {
  type: 'capability' as const,
  agentId: 'utils',
  capabilityId: 'random-double',
  search: 'Random Double',
};
