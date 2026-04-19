import { test, expect } from '@playwright/test';

/**
 * Workflow Note Regression Test
 *
 * This test verifies that editing a note and saving the workflow
 * does not cause elements to disappear from the execution graph.
 *
 * Test steps:
 * 1. Open an existing workflow with multiple versions
 * 2. Capture the original execution graph (nodes and edges)
 * 3. Modify a note and save
 * 4. Compare the graph to detect any disappeared elements
 *
 * Note: Uses dynamic workflow selection from the workflows list
 * to ensure the test works regardless of which workflows exist.
 */

test.describe('Workflow Note Regression Tests', () => {
  // Increase timeout for these tests since they involve complex UI interactions
  test.setTimeout(120000);

  test('editing a note should not cause graph elements to disappear', async ({
    page,
  }) => {
    // Step 1: Navigate to workflows list and pick the first existing workflow
    await page.goto('/workflows');
    await page.waitForLoadState('networkidle');

    // Wait for workflows to load
    await expect(
      page.getByRole('heading', { name: /build and iterate automation flows/i })
    ).toBeVisible({ timeout: 30000 });

    // Wait for the skeleton loading to disappear and actual content to appear
    // The skeleton shows gray rectangles, actual content will have workflow names as links
    await page.waitForLoadState('networkidle');

    // Wait for at least one workflow link to appear (not just the "New workflow" button)
    // Workflows are displayed as cards/rows with links to /workflows/{id}
    const workflowCardSelector =
      'a[href^="/workflows/"]:not([href="/workflows/create"])';

    // Wait for either workflows to load OR network error to appear
    const workflowOrError = await Promise.race([
      page
        .waitForSelector(workflowCardSelector, {
          state: 'visible',
          timeout: 30000,
        })
        .then(() => 'workflows'),
      page
        .waitForSelector('text=Unable to connect to backend', {
          state: 'visible',
          timeout: 30000,
        })
        .then(() => 'network-error'),
      page
        .waitForSelector('text=Network Error', {
          state: 'visible',
          timeout: 30000,
        })
        .then(() => 'network-error'),
    ]).catch(() => 'timeout');

    if (workflowOrError === 'network-error') {
      await page.screenshot({
        path: 'e2e-results/screenshots/workflows-list-network-error.png',
        fullPage: true,
      });
      console.log('Backend API is not available - network error detected');
      test.skip(true, 'Backend API is not available');
      return;
    }

    if (workflowOrError === 'timeout') {
      // Take a screenshot for debugging if no workflows found
      await page.screenshot({
        path: 'e2e-results/screenshots/workflows-list-debug.png',
        fullPage: true,
      });
      console.log('No workflows found after waiting');
      test.skip(true, 'No existing workflows found to test');
      return;
    }

    // Take a screenshot for debugging
    await page.screenshot({
      path: 'e2e-results/screenshots/workflows-list-debug.png',
      fullPage: true,
    });

    const workflowLinks = page.locator(workflowCardSelector);
    const workflowCount = await workflowLinks.count();
    console.log(`Found ${workflowCount} workflow links`);

    if (workflowCount === 0) {
      test.skip(true, 'No existing workflows found to test');
      return;
    }

    // Click the first workflow
    await workflowLinks.first().click();
    await page.waitForLoadState('networkidle');

    // Wait for the workflow editor to fully load
    await expect(page.locator('.react-flow')).toBeVisible({ timeout: 30000 });

    // Wait for the workflow to load - check that the title is not "Untitled Workflow"
    // This ensures the actual workflow data has loaded
    await page.waitForFunction(
      () => {
        const title = document.querySelector('span.text-lg.font-semibold');
        return (
          title && title.textContent && !title.textContent.includes('Untitled')
        );
      },
      { timeout: 30000 }
    );

    // Log the workflow name for debugging
    const workflowName = await page
      .locator('span.text-lg.font-semibold')
      .textContent();
    console.log(`Testing with workflow: ${workflowName}`);

    // Step 2: Check if version selector exists (workflow has versions)
    const versionButton = page
      .locator('button')
      .filter({ has: page.locator('svg.lucide-chevrons-up-down') })
      .first();
    const hasVersions = await versionButton.isVisible().catch(() => false);

    if (hasVersions) {
      // Get current version text for later verification
      const currentVersionText = await versionButton.textContent();
      console.log(`Current version: ${currentVersionText}`);
    } else {
      console.log('Workflow has no versions or version selector not visible');
    }

    // Step 3: Capture the original execution graph state
    // Get all nodes (excluding create nodes which are for adding new steps)
    const originalNodes = await page.locator('.react-flow__node').all();
    const originalNodeCount = originalNodes.length;

    // Capture node IDs and types
    const originalNodeInfo: { id: string; type: string | null }[] = [];
    for (const node of originalNodes) {
      const nodeId = await node.getAttribute('data-id');
      const nodeType = await node.getAttribute('class');
      originalNodeInfo.push({ id: nodeId || '', type: nodeType });
    }

    // Get all edges
    const originalEdges = await page.locator('.react-flow__edge').all();
    const originalEdgeCount = originalEdges.length;

    // Capture edge info
    const originalEdgeInfo: { id: string | null }[] = [];
    for (const edge of originalEdges) {
      const edgeId = await edge.getAttribute('data-id');
      originalEdgeInfo.push({ id: edgeId });
    }

    console.log(
      `Original graph state: ${originalNodeCount} nodes, ${originalEdgeCount} edges`
    );
    console.log(
      'Original nodes:',
      originalNodeInfo.map((n) => n.id).join(', ')
    );
    console.log(
      'Original edges:',
      originalEdgeInfo.map((e) => e.id).join(', ')
    );

    // Step 4: Find a note node and modify it
    const noteNodes = page.locator('.react-flow__node').filter({
      has: page.locator('.bg-yellow-50, [class*="bg-yellow"]'),
    });

    const noteCount = await noteNodes.count();

    if (noteCount === 0) {
      // If no notes exist, add one
      console.log('No notes found, adding a new note');

      // Click the Add Note button (StickyNote icon button)
      const addNoteButton = page
        .locator('button')
        .filter({ has: page.locator('svg.lucide-sticky-note') });
      await addNoteButton.click();

      // Wait for the note to be added
      await page.waitForTimeout(500);

      // Now find the newly created note
      const newNoteNode = page
        .locator('.react-flow__node')
        .filter({
          has: page.locator('.bg-yellow-50, [class*="bg-yellow"]'),
        })
        .first();

      await expect(newNoteNode).toBeVisible();

      // Double-click to enter edit mode
      await newNoteNode.dblclick();

      // Type some content
      const textarea = page.locator('textarea').first();
      await textarea.fill('Test note content - ' + Date.now());

      // Click outside to exit edit mode
      await page.locator('.react-flow').click({ position: { x: 10, y: 10 } });
    } else {
      console.log(`Found ${noteCount} note(s), modifying the first one`);

      // Get the first note node
      const firstNote = noteNodes.first();
      await firstNote.scrollIntoViewIfNeeded();

      // Double-click to enter edit mode
      await firstNote.dblclick();

      // Wait for the textarea to appear
      await expect(page.locator('textarea').first()).toBeVisible({
        timeout: 5000,
      });

      // Get current content and modify it
      const textarea = page.locator('textarea').first();
      const currentContent = await textarea.inputValue();

      // Append timestamp to the note
      await textarea.fill(
        `${currentContent}\n\nModified at: ${new Date().toISOString()}`
      );

      // Click outside to exit edit mode and trigger save to store
      await page.locator('.react-flow').click({ position: { x: 10, y: 10 } });
    }

    // Wait a moment for the change to be registered
    await page.waitForTimeout(500);

    // Step 5: Save the workflow
    // The Save button should now be enabled (isDirty should be true)
    const saveButton = page
      .locator('button[type="submit"]')
      .filter({ has: page.locator('svg.lucide-save') });

    // Wait for the save button to be enabled
    await expect(saveButton).toBeEnabled({ timeout: 5000 });

    // Click save
    await saveButton.click();

    // Wait for the save to complete
    await page.waitForLoadState('networkidle');

    // Wait for toast notification indicating successful save
    const toast = page
      .locator('[data-sonner-toast]')
      .filter({ hasText: /updated|saved/i });
    await expect(toast).toBeVisible({ timeout: 10000 });

    // Step 6: Wait for the UI to stabilize after save
    await page.waitForTimeout(1000);

    // Step 7: Capture the graph state after save
    const afterSaveNodes = await page.locator('.react-flow__node').all();
    const afterSaveNodeCount = afterSaveNodes.length;

    const afterSaveNodeInfo: { id: string; type: string | null }[] = [];
    for (const node of afterSaveNodes) {
      const nodeId = await node.getAttribute('data-id');
      const nodeType = await node.getAttribute('class');
      afterSaveNodeInfo.push({ id: nodeId || '', type: nodeType });
    }

    const afterSaveEdges = await page.locator('.react-flow__edge').all();
    const afterSaveEdgeCount = afterSaveEdges.length;

    const afterSaveEdgeInfo: { id: string | null }[] = [];
    for (const edge of afterSaveEdges) {
      const edgeId = await edge.getAttribute('data-id');
      afterSaveEdgeInfo.push({ id: edgeId });
    }

    console.log(
      `After save graph state: ${afterSaveNodeCount} nodes, ${afterSaveEdgeCount} edges`
    );
    console.log(
      'After save nodes:',
      afterSaveNodeInfo.map((n) => n.id).join(', ')
    );
    console.log(
      'After save edges:',
      afterSaveEdgeInfo.map((e) => e.id).join(', ')
    );

    // Step 8: Compare and identify any disappeared elements
    const originalNodeIds = new Set(originalNodeInfo.map((n) => n.id));
    const afterSaveNodeIds = new Set(afterSaveNodeInfo.map((n) => n.id));

    const disappearedNodes = [...originalNodeIds].filter(
      (id) => !afterSaveNodeIds.has(id)
    );
    const newNodes = [...afterSaveNodeIds].filter(
      (id) => !originalNodeIds.has(id)
    );

    const originalEdgeIds = new Set(originalEdgeInfo.map((e) => e.id));
    const afterSaveEdgeIds = new Set(afterSaveEdgeInfo.map((e) => e.id));

    const disappearedEdges = [...originalEdgeIds].filter(
      (id) => !afterSaveEdgeIds.has(id)
    );
    const newEdges = [...afterSaveEdgeIds].filter(
      (id) => !originalEdgeIds.has(id)
    );

    // Log any changes
    if (disappearedNodes.length > 0) {
      console.error('DISAPPEARED NODES:', disappearedNodes);
    }
    if (newNodes.length > 0) {
      console.log('New nodes added:', newNodes);
    }
    if (disappearedEdges.length > 0) {
      console.error('DISAPPEARED EDGES:', disappearedEdges);
    }
    if (newEdges.length > 0) {
      console.log('New edges added:', newEdges);
    }

    // Step 9: Assert that no nodes disappeared (except potentially the one we just modified)
    // Note: We allow for small variations due to dynamic elements like CreateNode
    const significantNodeLoss = disappearedNodes.filter(
      (id) => !id.includes('create') && id !== 'start' && id.length > 0
    );

    expect(
      significantNodeLoss.length,
      `Expected no significant nodes to disappear, but found: ${significantNodeLoss.join(', ')}`
    ).toBe(0);

    // Assert that no edges disappeared
    const significantEdgeLoss = disappearedEdges.filter(
      (id) => id && id.length > 0
    );

    expect(
      significantEdgeLoss.length,
      `Expected no edges to disappear, but found: ${significantEdgeLoss.join(', ')}`
    ).toBe(0);

    // Take a screenshot for visual verification
    await page.screenshot({
      path: `e2e-results/screenshots/workflow-note-regression-after-save.png`,
      fullPage: true,
    });
  });

  test('verify execution graph structure after note modification', async ({
    page,
  }) => {
    // This test captures detailed graph structure for debugging

    // Navigate to workflows list and pick the first existing workflow
    await page.goto('/workflows');
    await page.waitForLoadState('networkidle');

    await expect(
      page.getByRole('heading', { name: /build and iterate automation flows/i })
    ).toBeVisible({ timeout: 30000 });
    await page.waitForLoadState('networkidle');

    const workflowCardSelector =
      'a[href^="/workflows/"]:not([href="/workflows/create"])';

    // Wait for either workflows to load OR network error to appear
    const workflowOrError = await Promise.race([
      page
        .waitForSelector(workflowCardSelector, {
          state: 'visible',
          timeout: 30000,
        })
        .then(() => 'workflows'),
      page
        .waitForSelector('text=Unable to connect to backend', {
          state: 'visible',
          timeout: 30000,
        })
        .then(() => 'network-error'),
      page
        .waitForSelector('text=Network Error', {
          state: 'visible',
          timeout: 30000,
        })
        .then(() => 'network-error'),
    ]).catch(() => 'timeout');

    if (workflowOrError === 'network-error') {
      console.log('Backend API is not available - network error detected');
      test.skip(true, 'Backend API is not available');
      return;
    }

    if (workflowOrError === 'timeout') {
      console.log('No workflows found after waiting');
      test.skip(true, 'No existing workflows found to test');
      return;
    }

    const workflowLinks = page.locator(workflowCardSelector);
    await workflowLinks.first().click();
    await page.waitForLoadState('networkidle');

    await expect(page.locator('.react-flow')).toBeVisible({ timeout: 30000 });

    // Wait for the workflow to load - check that the title is not "Untitled Workflow"
    await page.waitForFunction(
      () => {
        const title = document.querySelector('span.text-lg.font-semibold');
        return (
          title && title.textContent && !title.textContent.includes('Untitled')
        );
      },
      { timeout: 30000 }
    );

    const workflowName = await page
      .locator('span.text-lg.font-semibold')
      .textContent();
    console.log(`Testing execution graph with workflow: ${workflowName}`);

    // Intercept the API calls to capture the actual execution graph data
    let savedExecutionGraph: any = null;

    page.on('request', (request) => {
      if (request.method() === 'PUT' && request.url().includes('/workflows/')) {
        const postData = request.postData();
        if (postData) {
          try {
            savedExecutionGraph = JSON.parse(postData);
            console.log(
              'Captured save request:',
              JSON.stringify(savedExecutionGraph, null, 2).substring(0, 500)
            );
          } catch {
            console.log('Could not parse save request');
          }
        }
      }
    });

    // Capture initial state via Export functionality
    const exportButton = page.locator('button').filter({ hasText: 'Export' });

    // Set up download handler
    const [download] = await Promise.all([
      page.waitForEvent('download'),
      exportButton.click(),
    ]);

    const initialExport = await download.path();
    console.log('Initial export saved to:', initialExport);

    // Read the exported file content
    const fs = await import('fs');
    const initialGraphContent = fs.readFileSync(initialExport!, 'utf-8');
    const initialGraph = JSON.parse(initialGraphContent);

    console.log('Initial graph structure:');
    console.log(
      '- Steps:',
      Object.keys(initialGraph.executionGraph?.steps || {}).length
    );
    console.log(
      '- Execution plan transitions:',
      (initialGraph.executionGraph?.executionPlan || []).length
    );
    console.log('- Notes:', (initialGraph.executionGraph?.notes || []).length);
    console.log('- Entry point:', initialGraph.executionGraph?.entryPoint);

    // Modify a note
    const noteNodes = page.locator('.react-flow__node').filter({
      has: page.locator('.bg-yellow-50, [class*="bg-yellow"]'),
    });

    if ((await noteNodes.count()) > 0) {
      const firstNote = noteNodes.first();
      await firstNote.dblclick();

      const textarea = page.locator('textarea').first();
      await expect(textarea).toBeVisible({ timeout: 5000 });

      const currentContent = await textarea.inputValue();
      await textarea.fill(
        `${currentContent}\n\nTest modification: ${Date.now()}`
      );

      await page.locator('.react-flow').click({ position: { x: 10, y: 10 } });
      await page.waitForTimeout(500);
    }

    // Save
    const saveButton = page
      .locator('button[type="submit"]')
      .filter({ has: page.locator('svg.lucide-save') });
    await expect(saveButton).toBeEnabled({ timeout: 5000 });
    await saveButton.click();

    await page.waitForLoadState('networkidle');
    await page.waitForTimeout(2000);

    // Export again after save
    const [downloadAfter] = await Promise.all([
      page.waitForEvent('download'),
      exportButton.click(),
    ]);

    const afterExport = await downloadAfter.path();
    const afterGraphContent = fs.readFileSync(afterExport!, 'utf-8');
    const afterGraph = JSON.parse(afterGraphContent);

    console.log('\nAfter save graph structure:');
    console.log(
      '- Steps:',
      Object.keys(afterGraph.executionGraph?.steps || {}).length
    );
    console.log(
      '- Execution plan transitions:',
      (afterGraph.executionGraph?.executionPlan || []).length
    );
    console.log('- Notes:', (afterGraph.executionGraph?.notes || []).length);
    console.log('- Entry point:', afterGraph.executionGraph?.entryPoint);

    // Compare
    const initialSteps = Object.keys(initialGraph.executionGraph?.steps || {});
    const afterSteps = Object.keys(afterGraph.executionGraph?.steps || {});

    const missingSteps = initialSteps.filter((s) => !afterSteps.includes(s));
    const newSteps = afterSteps.filter((s) => !initialSteps.includes(s));

    if (missingSteps.length > 0) {
      console.error('MISSING STEPS after save:', missingSteps);
    }
    if (newSteps.length > 0) {
      console.log('New steps after save:', newSteps);
    }

    // Assert no steps were lost
    expect(
      missingSteps.length,
      `Expected no steps to be lost, but missing: ${missingSteps.join(', ')}`
    ).toBe(0);

    // Assert execution plan is preserved
    const initialTransitions =
      initialGraph.executionGraph?.executionPlan?.length || 0;
    const afterTransitions =
      afterGraph.executionGraph?.executionPlan?.length || 0;

    expect(
      afterTransitions,
      `Expected execution plan to be preserved. Was ${initialTransitions}, now ${afterTransitions}`
    ).toBeGreaterThanOrEqual(initialTransitions);
  });
});
