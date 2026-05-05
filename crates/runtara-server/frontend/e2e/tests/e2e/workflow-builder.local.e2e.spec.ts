import { test } from '@playwright/test';
import {
  randomDoubleStep,
  WorkflowBuilderPage,
} from '../../pages/WorkflowBuilderPage';

const runId = Date.now();

const stepType = (stepTypeName: string) =>
  ({
    type: 'step' as const,
    stepType: stepTypeName,
  }) as const;

test.describe.serial('Workflow builder local UI', () => {
  let createdWorkflowNames: string[] = [];

  const workflowName = (suffix: string) =>
    `E2E Workflow Builder ${suffix} ${runId}`;

  const rememberWorkflow = async (
    builder: WorkflowBuilderPage,
    name: string
  ) => {
    await builder.createWorkflow(name);
    createdWorkflowNames.push(name);
  };

  test.afterEach(async ({ page }) => {
    const builder = new WorkflowBuilderPage(page);

    for (const workflow of [...createdWorkflowNames].reverse()) {
      try {
        await builder.deleteWorkflowFromList(workflow);
      } catch {
        // Best-effort cleanup, still through UI only.
      }
    }

    createdWorkflowNames = [];
  });

  test('timeline-only adds, persists, and removes linear steps', async ({
    page,
  }) => {
    const builder = new WorkflowBuilderPage(page);
    await rememberWorkflow(builder, workflowName('timeline-linear'));

    await builder.addFirstTimelineStep(randomDoubleStep, {
      name: 'Generate number',
    });
    await builder.addTimelineStepAfter('Generate number', randomDoubleStep, {
      name: 'Double again',
    });

    await builder.saveWorkflow();
    await builder.reloadAndWait();
    await builder.expectStepVisible('Generate number');
    await builder.expectStepVisible('Double again');

    await builder.deleteTimelineStep('Double again');
    await builder.saveWorkflow();
    await builder.reloadAndWait();
    await builder.expectStepVisible('Generate number');
    await builder.expectStepHidden('Double again');
  });

  test('adds and removes conditional branch steps through the UI', async ({
    page,
  }) => {
    const builder = new WorkflowBuilderPage(page);
    await rememberWorkflow(builder, workflowName('conditional-branches'));

    await builder.beginFirstTimelineStep(stepType('Conditional'));
    await builder.configureOpenCondition();
    await builder.saveOpenTimelinePanel({ name: 'Decision' });
    await builder.addCanvasBranchStep('true', 'Decision', randomDoubleStep, {
      name: 'Approved branch',
    });
    await builder.addCanvasBranchStep('false', 'Decision', randomDoubleStep, {
      name: 'Rejected branch',
    });

    await builder.saveWorkflow();
    await builder.reloadAndWait();
    await builder.expectStepVisible('Decision');
    await builder.expectStepVisible('Approved branch');
    await builder.expectStepVisible('Rejected branch');

    await builder.deleteTimelineStep('Rejected branch');
    await builder.saveWorkflow();
    await builder.reloadAndWait();
    await builder.expectStepVisible('Approved branch');
    await builder.expectStepHidden('Rejected branch');
  });

  test('adds a split, nests a child step, and removes both through the UI', async ({
    page,
  }) => {
    const builder = new WorkflowBuilderPage(page);
    await rememberWorkflow(builder, workflowName('split-lifecycle'));

    await builder.addFirstTimelineStep(randomDoubleStep, {
      name: 'Prepare items',
    });
    await builder.addTimelineStepAfter('Prepare items', stepType('Split'), {
      name: 'For each item',
    });
    await builder.configureSplitSource('For each item');
    await builder.addNestedTimelineStep('For each item', randomDoubleStep, {
      name: 'Process item',
    });
    await builder.addTimelineStepAfter('Process item', randomDoubleStep, {
      name: 'Finalize item',
    });

    await builder.saveWorkflow();
    await builder.reloadAndWait();
    await builder.expectStepVisible('Prepare items');
    await builder.expectStepVisible('For each item');
    await builder.expectStepVisible('Process item');
    await builder.expectStepVisible('Finalize item');
    await builder.expectStepNestedUnder('Process item', 'For each item');
    await builder.expectStepNestedUnder('Finalize item', 'For each item');

    await builder.deleteTimelineStep('Process item');
    await builder.saveWorkflow();
    await builder.reloadAndWait();
    await builder.expectStepVisible('Prepare items');
    await builder.expectStepVisible('For each item');
    await builder.expectStepVisible('Finalize item');
    await builder.expectStepHidden('Process item');

    await builder.deleteTimelineStep('For each item');
    await builder.saveWorkflow();
    await builder.reloadAndWait();
    await builder.expectStepVisible('Prepare items');
    await builder.expectStepHidden('For each item');
    await builder.expectStepHidden('Finalize item');
  });

  test('timeline-only nests a split inside a while loop', async ({ page }) => {
    const builder = new WorkflowBuilderPage(page);
    await rememberWorkflow(builder, workflowName('while-with-split'));

    await builder.beginFirstTimelineStep(stepType('While'));
    await builder.configureOpenCondition();
    await builder.saveOpenTimelinePanel({ name: 'Retry loop' });
    await builder.addNestedTimelineStep('Retry loop', stepType('Split'), {
      name: 'Loop items',
    });
    await builder.configureSplitSource('Loop items');
    await builder.addNestedTimelineStep('Loop items', randomDoubleStep, {
      name: 'Loop item work',
    });

    await builder.saveWorkflow();
    await builder.reloadAndWait();
    await builder.expectStepVisible('Retry loop');
    await builder.expectStepVisible('Loop items');
    await builder.expectStepVisible('Loop item work');
    await builder.expectStepNestedUnder('Loop items', 'Retry loop');
    await builder.expectStepNestedUnder('Loop item work', 'Loop items');
  });

  test('removes a while loop and its nested descendants', async ({ page }) => {
    const builder = new WorkflowBuilderPage(page);
    await rememberWorkflow(builder, workflowName('while-descendant-remove'));

    await builder.addFirstTimelineStep(randomDoubleStep, {
      name: 'Stable root',
    });
    await builder.beginTimelineStepAfter('Stable root', stepType('While'));
    await builder.configureOpenCondition();
    await builder.saveOpenTimelinePanel({ name: 'Removable loop' });
    await builder.addNestedTimelineStep('Removable loop', randomDoubleStep, {
      name: 'Loop child',
    });
    await builder.addTimelineStepAfter('Loop child', randomDoubleStep, {
      name: 'Loop descendant',
    });

    await builder.saveWorkflow();
    await builder.reloadAndWait();
    await builder.expectStepVisible('Stable root');
    await builder.expectStepVisible('Removable loop');
    await builder.expectStepVisible('Loop child');
    await builder.expectStepVisible('Loop descendant');
    await builder.expectStepNestedUnder('Loop child', 'Removable loop');
    await builder.expectStepNestedUnder('Loop descendant', 'Removable loop');

    await builder.deleteTimelineStep('Removable loop');
    await builder.saveWorkflow();
    await builder.reloadAndWait();
    await builder.expectStepVisible('Stable root');
    await builder.expectStepHidden('Removable loop');
    await builder.expectStepHidden('Loop child');
    await builder.expectStepHidden('Loop descendant');
  });

  test('persists deeper while and split nesting parents', async ({ page }) => {
    const builder = new WorkflowBuilderPage(page);
    await rememberWorkflow(builder, workflowName('deep-nesting'));

    await builder.beginFirstTimelineStep(stepType('While'));
    await builder.configureOpenCondition();
    await builder.saveOpenTimelinePanel({ name: 'Outer while' });
    await builder.addNestedTimelineStep('Outer while', stepType('Split'), {
      name: 'Outer split',
    });
    await builder.configureSplitSource('Outer split', 'data.outerItems');
    await builder.addNestedTimelineStep('Outer split', stepType('Split'), {
      name: 'Inner split',
    });
    await builder.configureSplitSource('Inner split', 'data.innerItems');
    await builder.addNestedTimelineStep('Inner split', randomDoubleStep, {
      name: 'Deep child',
    });
    await builder.addTimelineStepAfter('Inner split', randomDoubleStep, {
      name: 'Outer split survivor',
    });

    await builder.saveWorkflow();
    await builder.reloadAndWait();
    await builder.expectStepNestedUnder('Outer split', 'Outer while');
    await builder.expectStepNestedUnder('Inner split', 'Outer split');
    await builder.expectStepNestedUnder('Deep child', 'Inner split');
    await builder.expectStepNestedUnder('Outer split survivor', 'Outer split');

    await builder.deleteTimelineStep('Inner split');
    await builder.saveWorkflow();
    await builder.reloadAndWait();
    await builder.expectStepVisible('Outer while');
    await builder.expectStepVisible('Outer split');
    await builder.expectStepVisible('Outer split survivor');
    await builder.expectStepHidden('Inner split');
    await builder.expectStepHidden('Deep child');
    await builder.expectStepNestedUnder('Outer split', 'Outer while');
    await builder.expectStepNestedUnder('Outer split survivor', 'Outer split');
  });

  test('reorders timeline root and nested steps with drag handles', async ({
    page,
  }) => {
    const builder = new WorkflowBuilderPage(page);
    await rememberWorkflow(builder, workflowName('timeline-reorder'));

    await builder.addFirstTimelineStep(randomDoubleStep, { name: 'Root A' });
    await builder.addTimelineStepAfter('Root A', randomDoubleStep, {
      name: 'Root B',
    });
    await builder.addTimelineStepAfter('Root B', randomDoubleStep, {
      name: 'Root C',
    });
    await builder.beginTimelineStepAfter('Root C', stepType('While'));
    await builder.configureOpenCondition();
    await builder.saveOpenTimelinePanel({ name: 'Root Loop' });
    await builder.addNestedTimelineStep('Root Loop', randomDoubleStep, {
      name: 'Nested 1',
    });
    await builder.addTimelineStepAfter('Nested 1', randomDoubleStep, {
      name: 'Nested 2',
    });

    await builder.dragTimelineStep('Root C', 'Root A', 'before');
    await builder.dragTimelineStep('Nested 2', 'Nested 1', 'before');

    await builder.saveWorkflow();
    await builder.reloadAndWait();
    await builder.expectTimelineOrder([
      'Root C',
      'Root A',
      'Root B',
      'Root Loop',
    ]);
    await builder.expectTimelineChildOrder('Root Loop', [
      'Nested 2',
      'Nested 1',
    ]);
  });

  test('shows validation for empty split and invalid while saves', async ({
    page,
  }) => {
    const builder = new WorkflowBuilderPage(page);
    await rememberWorkflow(builder, workflowName('invalid-containers'));

    await builder.addFirstTimelineStep(stepType('Split'), {
      name: 'Empty split',
    });
    await builder.expectSaveValidationFailure();
    await builder.deleteTimelineStep('Empty split');
    await builder.clearValidationMessages();

    await builder.beginFirstTimelineStep(stepType('While'));
    await builder.saveOpenTimelinePanel({ name: 'Invalid while' });
    await builder.expectSaveValidationFailure();
    await builder.deleteTimelineStep('Invalid while');
  });

  test('calls another workflow from a parent workflow through the UI', async ({
    page,
  }) => {
    const builder = new WorkflowBuilderPage(page);
    const childWorkflowName = workflowName('called-child');
    const parentWorkflowName = workflowName('calling-parent');

    await rememberWorkflow(builder, childWorkflowName);
    await builder.addFirstTimelineStep(randomDoubleStep, {
      name: 'Child work',
    });
    await builder.saveWorkflow();

    await rememberWorkflow(builder, parentWorkflowName);
    await builder.beginFirstTimelineStep(stepType('EmbedWorkflow'));
    await builder.configureOpenEmbedWorkflow(childWorkflowName);
    await builder.saveOpenTimelinePanel({ name: 'Call child workflow' });

    await builder.saveWorkflow();
    await builder.reloadAndWait();
    await builder.expectStepVisible('Call child workflow');
  });

  test('persists a specific embedded child workflow version', async ({
    page,
  }) => {
    const builder = new WorkflowBuilderPage(page);
    const childWorkflowName = workflowName('versioned-child');
    const parentWorkflowName = workflowName('versioned-parent');

    await rememberWorkflow(builder, childWorkflowName);
    await builder.addFirstTimelineStep(randomDoubleStep, {
      name: 'Child v2 work',
    });
    await builder.saveWorkflow();
    await builder.addTimelineStepAfter('Child v2 work', randomDoubleStep, {
      name: 'Child v3 work',
    });
    await builder.saveWorkflow();

    await rememberWorkflow(builder, parentWorkflowName);
    await builder.beginFirstTimelineStep(stepType('EmbedWorkflow'));
    await builder.configureOpenEmbedWorkflow(childWorkflowName);
    await builder.configureOpenEmbedWorkflowVersion(2);
    await builder.saveOpenTimelinePanel({ name: 'Call child v2' });

    await builder.saveWorkflow();
    await builder.reloadAndWait();
    await builder.editTimelineStep('Call child v2');
    await builder.expectOpenEmbedWorkflowVersion(2);
  });

  test('uses canvas controls for add, branch removal, and nested containers', async ({
    page,
  }) => {
    const builder = new WorkflowBuilderPage(page);
    await rememberWorkflow(builder, workflowName('canvas-only'));

    await builder.addFirstCanvasStep(randomDoubleStep, {
      name: 'Canvas start',
    });
    await builder.beginCanvasStepAfter('Canvas start', stepType('While'));
    await builder.configureOpenCanvasCondition();
    await builder.saveOpenCanvasDialog({ name: 'Canvas loop' });
    await builder.beginNestedCanvasStep('Canvas loop', stepType('Split'));
    await builder.configureOpenCanvasSplitSource('data.loopItems');
    await builder.saveOpenCanvasDialog({ name: 'Canvas split' });
    await builder.addNestedCanvasStep('Canvas split', randomDoubleStep, {
      name: 'Canvas temporary nested',
    });
    await builder.deleteCanvasStep('Canvas temporary nested');
    await builder.addNestedCanvasStep('Canvas split', randomDoubleStep, {
      name: 'Canvas nested work',
    });
    await builder.beginCanvasStepAfter('Canvas loop', stepType('Conditional'));
    await builder.configureOpenCanvasCondition();
    await builder.saveOpenCanvasDialog({ name: 'Canvas decision' });
    await builder.addCanvasBranchStep(
      'true',
      'Canvas decision',
      randomDoubleStep,
      {
        name: 'Canvas approved',
      }
    );
    await builder.addCanvasBranchStep(
      'false',
      'Canvas decision',
      randomDoubleStep,
      {
        name: 'Canvas rejected',
      }
    );
    await builder.deleteCanvasStep('Canvas rejected');

    await builder.saveWorkflow();
    await builder.reloadAndWait();
    await builder.expectStepVisible('Canvas start');
    await builder.expectStepVisible('Canvas loop');
    await builder.expectStepVisible('Canvas split');
    await builder.expectStepVisible('Canvas nested work');
    await builder.expectStepVisible('Canvas decision');
    await builder.expectStepVisible('Canvas approved');
    await builder.expectStepHidden('Canvas rejected');
    await builder.expectStepHidden('Canvas temporary nested');
    await builder.expectStepNestedUnder('Canvas split', 'Canvas loop');
    await builder.expectStepNestedUnder('Canvas nested work', 'Canvas split');
  });
});
