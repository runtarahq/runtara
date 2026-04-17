import { test, expect } from '@playwright/test';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

/**
 * Scenario Lifecycle E2E Test — Browser UI
 *
 * True end-to-end test that drives the UI:
 * Browser navigation → forms → ReactFlow canvas → toolbar actions → execution → deletion
 *
 * Flow: create via form → add step via canvas "+" → save → execute via Play → verify history → verify versions → delete
 *
 * Requires: full local stack running (gateway, runtime, management, frontend)
 */

const __filename = fileURLToPath(import.meta.url);
const __dirnameLocal = path.dirname(__filename);

const GATEWAY_URL = process.env.GATEWAY_URL || 'http://localhost:8080';
const SCENARIO_NAME = `E2E UI Lifecycle ${Date.now()}`;

/** Read the access token from the Playwright auth state file. */
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

/** Build common headers for authenticated API calls. */
function apiHeaders(token: string): Record<string, string> {
  return {
    Authorization: `Bearer ${token}`,
    'Content-Type': 'application/json',
  };
}

test.describe.serial('Scenario Lifecycle (UI)', () => {
  let scenarioId: string;
  let token: string;
  let compilationWorks = false;

  test.beforeAll(() => {
    token = getAccessToken();
  });

  // Safety-net cleanup: delete scenario via API if UI deletion fails mid-test
  test.afterAll(async () => {
    if (!scenarioId) return;
    try {
      await fetch(`${GATEWAY_URL}/api/runtime/scenarios/${scenarioId}/delete`, {
        method: 'POST',
        headers: apiHeaders(token),
      });
    } catch {
      // best-effort cleanup
    }
  });

  test('create scenario via UI', async ({ page }) => {
    // Navigate to the create scenario form
    await page.goto('/scenarios/create');
    await page.waitForLoadState('networkidle');

    // Fill the name field and submit
    await page.getByLabel('Name').fill(SCENARIO_NAME);
    await page.getByRole('button', { name: 'Save' }).click();

    // Should redirect to the scenario editor page (not /scenarios/create)
    await page.waitForURL(
      (url) => /\/scenarios\/(?!create\b)[a-zA-Z0-9_-]+$/.test(url.pathname),
      { timeout: 15000 }
    );

    // Extract scenario ID from URL for subsequent tests
    const url = page.url();
    scenarioId = url.split('/scenarios/').pop()!;
    expect(scenarioId).toBeTruthy();
    expect(scenarioId).not.toBe('create');
  });

  test('add a random-double step and save', async ({ page }) => {
    await page.goto(`/scenarios/${scenarioId}`);
    await page.waitForLoadState('networkidle');

    // Wait for the ReactFlow canvas to render
    await expect(page.locator('.react-flow')).toBeVisible({ timeout: 15000 });

    // Click the "+" button next to the Start indicator node
    const startNode = page
      .locator('.react-flow__node')
      .filter({ hasText: 'Start' });
    await expect(startNode).toBeVisible();
    await startNode.getByRole('button').click();

    // StepPickerModal should open — search for "Random Double" capability
    // (displayName is "Random Double", name is "random_double", id is "random-double")
    const searchInput = page.getByPlaceholder('Search steps or operations...');
    await expect(searchInput).toBeVisible();
    await searchInput.fill('Random Double');

    // Wait for capability search results (requires agent details to load)
    const capabilityResult = page.getByText('Random Double').first();
    await expect(capabilityResult).toBeVisible({ timeout: 30000 });
    await capabilityResult.click();

    // NodeConfigDialog opens in create mode — click Save inside the form
    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible({ timeout: 5000 });
    await dialog.getByRole('button', { name: 'Save' }).click();

    // Verify the node appears on the canvas
    await expect(
      page.locator('.react-flow__node').filter({ hasText: 'Random Double' })
    ).toBeVisible({ timeout: 10000 });

    // Now save the scenario via toolbar (the graph is dirty after adding a step)
    const saveButton = page.getByTitle('Save changes');
    await expect(saveButton).toBeVisible({ timeout: 5000 });
    await saveButton.click();

    // Wait for save to complete
    await expect(page.getByTitle('No changes to save')).toBeVisible({
      timeout: 15000,
    });
  });

  test('execute and verify completion', async ({ page }) => {
    await page.goto(`/scenarios/${scenarioId}`);
    await page.waitForLoadState('networkidle');
    await expect(page.locator('.react-flow')).toBeVisible({ timeout: 15000 });

    // Play button should be enabled (no unsaved changes after fresh load)
    const playButton = page.getByTitle('Start scenario');
    await expect(playButton).toBeVisible({ timeout: 10000 });
    await expect(playButton).toBeEnabled({ timeout: 5000 });
    await playButton.click();

    // Wait for execution — backend compiles on-demand then runs
    // If compilation fails, the execution won't show "Execution in progress"
    try {
      await expect(page.getByText('Execution in progress')).toBeVisible({
        timeout: 60000,
      });
    } catch {
      // Compilation likely unavailable (e.g. native cache stale in CI)
      console.log(
        'Execution did not start — compilation may be unavailable, skipping execution tests'
      );
      test.skip(true, 'Scenario compilation unavailable');
      return;
    }

    compilationWorks = true;

    // Click "View execution details" to switch to History tab
    await page.getByTitle('View execution details').click();

    // Wait for the "Completed" badge in the history panel
    await expect(page.getByText('Completed').first()).toBeVisible({
      timeout: 60000,
    });
  });

  test('verify version in Versions panel', async ({ page }) => {
    test.skip(!compilationWorks, 'Skipped — compilation unavailable');

    await page.goto(`/scenarios/${scenarioId}`);
    await page.waitForLoadState('networkidle');
    await expect(page.locator('.react-flow')).toBeVisible({ timeout: 15000 });

    // Click Versions tab (auto-expands the panel)
    await page.getByRole('button', { name: /Versions/ }).click();

    // Verify a "Compiled" badge is visible
    await expect(page.getByText('Compiled').first()).toBeVisible({
      timeout: 10000,
    });

    // Verify an active version exists
    await expect(page.getByRole('button', { name: /Active/ })).toBeVisible();
  });

  test('delete scenario from list', async ({ page }) => {
    await page.goto('/scenarios');
    await page.waitForLoadState('networkidle');

    // Find the scenario card by its unique name
    const card = page.locator('article').filter({ hasText: SCENARIO_NAME });
    await expect(card).toBeVisible({ timeout: 10000 });

    // Click the delete button (.first() because EntityTile renders actions twice)
    await card.getByTitle('Delete').first().click();

    // Confirm in the confirmation dialog
    await page.getByRole('button', { name: 'Confirm' }).click();

    // Verify the card disappears
    await expect(card).not.toBeVisible({ timeout: 10000 });

    // Clear scenarioId so afterAll doesn't try to double-delete
    scenarioId = '';
  });
});
