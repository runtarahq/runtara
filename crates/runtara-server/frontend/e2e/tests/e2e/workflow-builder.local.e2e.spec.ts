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
});
