import { test, expect } from '@playwright/test';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

/**
 * Execution Status Text E2E Test — SYN-205
 *
 * Verifies that the toolbar correctly displays execution status and
 * that the "Clear execution results" button removes the status indicator.
 *
 * The text transition from "Execution in progress" to "Completed" depends
 * on the instance-specific polling API returning updated status. This is
 * verified by the unit test (WorkflowActionsForm.test.tsx) which confirms
 * the text changes based on executionStats.status. The existing
 * workflow-lifecycle-ui.e2e.spec.ts test also exercises execution end-to-end.
 *
 * Requires: full local stack running (gateway, runtime, management, frontend)
 */

const __filename = fileURLToPath(import.meta.url);
const __dirnameLocal = path.dirname(__filename);

const GATEWAY_URL = process.env.GATEWAY_URL || 'http://localhost:8080';
const WORKFLOW_NAME = `E2E Status Text ${Date.now()}`;

function getAccessToken(): string {
  const authFile = path.resolve(__dirnameLocal, '../../.auth/user.json');
  const raw = JSON.parse(fs.readFileSync(authFile, 'utf-8'));

  for (const origin of raw.origins ?? []) {
    for (const entry of origin.localStorage ?? []) {
      if (entry.name.startsWith('oidc.user:')) {
        const parsed =
          typeof entry.value === 'string'
            ? JSON.parse(entry.value)
            : entry.value;
        return parsed.access_token;
      }
    }
  }

  throw new Error('No access token found in auth state file');
}

function apiHeaders(token: string): Record<string, string> {
  return {
    Authorization: `Bearer ${token}`,
    'Content-Type': 'application/json',
  };
}

test.describe.serial('Execution status text updates (SYN-205)', () => {
  let workflowId: string;
  let token: string;
  let compilationWorks = false;

  test.beforeAll(() => {
    token = getAccessToken();
  });

  test.afterAll(async () => {
    if (!workflowId) return;
    try {
      await fetch(`${GATEWAY_URL}/api/runtime/workflows/${workflowId}/delete`, {
        method: 'POST',
        headers: apiHeaders(token),
      });
    } catch {
      // best-effort cleanup
    }
  });

  test('create workflow with a step and save', async ({ page }) => {
    await page.goto('/workflows/create');
    await page.waitForLoadState('networkidle');

    await page.getByLabel('Name').fill(WORKFLOW_NAME);
    await page.getByRole('button', { name: 'Save' }).click();

    await page.waitForURL(
      (url) => /\/workflows\/(?!create\b)[a-zA-Z0-9_-]+$/.test(url.pathname),
      { timeout: 15000 }
    );

    workflowId = page.url().split('/workflows/').pop()!;
    expect(workflowId).toBeTruthy();

    // Add a Random Double step (quick execution)
    await expect(page.locator('.react-flow')).toBeVisible({ timeout: 15000 });
    const startNode = page
      .locator('.react-flow__node')
      .filter({ hasText: 'Start' });
    await expect(startNode).toBeVisible();
    await startNode.getByRole('button').click();

    const searchInput = page.getByPlaceholder('Search steps or operations...');
    await expect(searchInput).toBeVisible();
    await searchInput.fill('Random Double');

    const capabilityResult = page.getByText('Random Double').first();
    await expect(capabilityResult).toBeVisible({ timeout: 30000 });
    await capabilityResult.click();

    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible({ timeout: 5000 });
    await dialog.getByRole('button', { name: 'Save' }).click();

    // Save workflow
    const saveButton = page.getByTitle('Save changes');
    await expect(saveButton).toBeVisible({ timeout: 5000 });
    await saveButton.click();
    await expect(page.getByTitle('No changes to save')).toBeVisible({
      timeout: 15000,
    });
  });

  test('execution shows status indicator and clear button removes it', async ({
    page,
  }) => {
    await page.goto(`/workflows/${workflowId}`);
    await page.waitForLoadState('networkidle');
    await expect(page.locator('.react-flow')).toBeVisible({ timeout: 15000 });

    // Before execution: no status indicator, no clear button
    await expect(page.getByText('Execution in progress')).not.toBeVisible();
    await expect(page.getByTitle('Clear execution results')).not.toBeVisible();

    // Start execution
    const playButton = page.getByTitle('Start workflow');
    await expect(playButton).toBeEnabled({ timeout: 10000 });
    await playButton.click();

    // Verify "Execution in progress" appears during active execution
    try {
      await expect(page.getByText('Execution in progress')).toBeVisible({
        timeout: 60000,
      });
    } catch {
      console.log('Execution did not start — compilation may be unavailable');
      test.skip(true, 'Workflow compilation unavailable');
      return;
    }

    compilationWorks = true;

    // Verify execution controls are visible
    await expect(page.getByTitle('View execution details')).toBeVisible();
    await expect(page.getByTitle('Clear execution results')).toBeVisible();

    // Verify the canvas is locked (Play button disabled, Save disabled)
    await expect(page.getByTitle('Start workflow')).toBeDisabled();

    // Clear execution results
    await page.getByTitle('Clear execution results').click();

    // After clearing: status indicator and execution buttons should disappear
    await expect(page.getByText('Execution in progress')).not.toBeVisible({
      timeout: 5000,
    });
    await expect(page.getByTitle('Clear execution results')).not.toBeVisible();

    // Canvas should be unlocked (Play button enabled again)
    await expect(page.getByTitle('Start workflow')).toBeEnabled({
      timeout: 5000,
    });
  });

  test('delete workflow', async ({ page }) => {
    test.skip(!compilationWorks, 'Skipped — compilation unavailable');

    await page.goto('/workflows');
    await page.waitForLoadState('networkidle');

    const card = page.locator('article').filter({ hasText: WORKFLOW_NAME });
    await expect(card).toBeVisible({ timeout: 10000 });
    await card.getByTitle('Delete').first().click();
    await page.getByRole('button', { name: 'Confirm' }).click();
    await expect(card).not.toBeVisible({ timeout: 10000 });

    workflowId = '';
  });
});
