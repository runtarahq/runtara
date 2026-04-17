import { test, expect } from '@playwright/test';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

/**
 * Problem Tab Step Reference E2E Test (SYN-234)
 *
 * Verifies that when a scenario is saved with a step missing required configuration
 * (e.g., Group By without config), the Problem tab shows an ERROR (not just a warning)
 * that references the correct step name ("Group By").
 *
 * Currently the backend returns a flat string error "Invalid scenario format: missing
 * field 'config'" with no step context, so the Problem tab only shows client-side
 * warnings (which may reference the wrong step). The fix should make the backend
 * return a structured validation error with the step ID.
 *
 * Requires: full local stack running (gateway, runtime, management, frontend)
 */

const __filename = fileURLToPath(import.meta.url);
const __dirnameLocal = path.dirname(__filename);

const GATEWAY_URL = process.env.GATEWAY_URL || 'http://localhost:8080';
const SCENARIO_NAME = `E2E Step Ref ${Date.now()}`;

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

test.describe.serial('Problem tab shows correct step name (SYN-234)', () => {
  let scenarioId: string;
  let token: string;

  test.beforeAll(() => {
    token = getAccessToken();
  });

  // Safety-net cleanup: delete scenario via API if UI deletion fails
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

  test('create scenario', async ({ page }) => {
    await page.goto('/scenarios/create');
    await page.waitForLoadState('networkidle');

    await page.getByLabel('Name').fill(SCENARIO_NAME);
    await page.getByRole('button', { name: 'Save' }).click();

    await page.waitForURL(
      (url) => /\/scenarios\/(?!create\b)[a-zA-Z0-9_-]+$/.test(url.pathname),
      { timeout: 15000 }
    );

    scenarioId = page.url().split('/scenarios/').pop()!;
    expect(scenarioId).toBeTruthy();
  });

  test('add Group By step without config, save, and verify error references correct step', async ({
    page,
  }) => {
    await page.goto(`/scenarios/${scenarioId}`);
    await page.waitForLoadState('networkidle');
    await expect(page.locator('.react-flow')).toBeVisible({ timeout: 15000 });

    // Add a Group By step via "+" on Start node
    const startNode = page
      .locator('.react-flow__node')
      .filter({ hasText: 'Start' });
    await expect(startNode).toBeVisible();
    await startNode.getByRole('button').click();

    const searchInput = page.getByPlaceholder('Search steps or operations...');
    await expect(searchInput).toBeVisible();
    await searchInput.fill('Group By');

    const capabilityResult = page.getByText('Group By').first();
    await expect(capabilityResult).toBeVisible({ timeout: 30000 });
    await capabilityResult.click();

    // Save the step in the config dialog WITHOUT filling required fields
    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible({ timeout: 5000 });
    await dialog.getByRole('button', { name: 'Save' }).click();
    await expect(dialog).not.toBeVisible({ timeout: 5000 });

    // Verify Group By node appears on canvas
    await expect(
      page.locator('.react-flow__node').filter({ hasText: 'Group By' })
    ).toBeVisible({ timeout: 10000 });

    // Save the scenario — triggers backend validation error about missing config
    const saveButton = page.getByTitle('Save changes');
    await expect(saveButton).toBeVisible({ timeout: 5000 });
    await saveButton.click();

    // Wait for save response (error expected)
    await page.waitForTimeout(5000);

    // Open the Problems tab
    const problemsButton = page.getByRole('button', { name: /Problems/ });
    await problemsButton.click();
    await page.waitForTimeout(500);

    // KEY ASSERTION: The Problem tab should have an ERROR (red, severity=error)
    // about missing configuration that references the "Group By" step.
    // The Errors tab must exist and have at least 1 error.
    const errorsTab = page.getByRole('button', { name: /Errors \(\d+\)/ });
    await expect(errorsTab).toBeVisible({ timeout: 5000 });

    // Switch to Errors-only filter to isolate from warnings
    await errorsTab.click();
    await page.waitForTimeout(300);

    // Find an error message (red text) that mentions "config" or "configuration"
    const errorMessages = page.locator('.text-destructive');
    await expect(errorMessages.first()).toBeVisible({ timeout: 5000 });

    // The error about missing config should have a "Step: Group By" badge
    // This is the core assertion: the error references the CORRECT step
    const errorWithStep = page
      .locator('.space-y-0\\.5 > div')
      .filter({
        has: page.locator('.text-destructive'),
      })
      .filter({
        hasText: 'Step: Group By',
      });
    await expect(errorWithStep).toBeVisible({ timeout: 5000 });
  });

  test('delete scenario', async ({ page }) => {
    await page.goto('/scenarios');
    await page.waitForLoadState('networkidle');

    const card = page.locator('article').filter({ hasText: SCENARIO_NAME });
    await expect(card).toBeVisible({ timeout: 10000 });

    await card.getByTitle('Delete').first().click();
    await page.getByRole('button', { name: 'Confirm' }).click();

    await expect(card).not.toBeVisible({ timeout: 10000 });
    scenarioId = '';
  });
});
