import { test, expect } from '@playwright/test';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

/**
 * Error Step Validation E2E Test (SYN-204)
 *
 * Verifies that the Error step form enforces required fields (Error Code, Error Message)
 * and prevents saving when they are empty.
 *
 * Flow: create workflow → add Error step → attempt save with empty fields → verify validation errors →
 *       fill required fields → save successfully → delete workflow
 *
 * Requires: full local stack running (gateway, runtime, management, frontend)
 */

const __filename = fileURLToPath(import.meta.url);
const __dirnameLocal = path.dirname(__filename);

const GATEWAY_URL = process.env.GATEWAY_URL || 'http://localhost:8080';
const WORKFLOW_NAME = `E2E Step Validation ${Date.now()}`;

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

test.describe.serial('Error Step Validation (SYN-204)', () => {
  let workflowId: string;
  let token: string;

  test.beforeAll(() => {
    token = getAccessToken();
  });

  // Safety-net cleanup: delete workflow via API if UI deletion fails
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

  test('create workflow', async ({ page }) => {
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
  });

  test('Error step Save is blocked when required fields are empty', async ({
    page,
  }) => {
    await page.goto(`/workflows/${workflowId}`);
    await page.waitForLoadState('networkidle');
    await expect(page.locator('.react-flow')).toBeVisible({ timeout: 15000 });

    // Click "+" on the Start node to open step picker
    const startNode = page
      .locator('.react-flow__node')
      .filter({ hasText: 'Start' });
    await expect(startNode).toBeVisible();
    await startNode.getByRole('button').click();

    // Search for Error step type
    const searchInput = page.getByPlaceholder('Search steps or operations...');
    await expect(searchInput).toBeVisible();
    await searchInput.fill('Error');

    // Click the Error step type (use description to avoid matching workflow title)
    const errorResult = page.getByText('Emit a structured error');
    await expect(errorResult).toBeVisible({ timeout: 10000 });
    await errorResult.click();

    // NodeConfigDialog should open — Error Code and Error Message should be visible
    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible({ timeout: 5000 });
    await expect(dialog.getByText('Error Code *')).toBeVisible();
    await expect(dialog.getByText('Error Message *')).toBeVisible();

    // Try to save without filling required fields
    await dialog.getByRole('button', { name: 'Save' }).click();

    // Wait a beat for any async submission to complete
    await page.waitForTimeout(1000);

    // Dialog should still be open — save was blocked by validation
    await expect(dialog).toBeVisible();

    // Verify the form fields are still empty (didn't submit and reset)
    await expect(dialog.getByPlaceholder('Enter error code...')).toHaveValue(
      ''
    );
    await expect(dialog.getByPlaceholder('Enter error message...')).toHaveValue(
      ''
    );

    // Validation error messages should appear
    await expect(dialog.getByText('Error Code is required.')).toBeVisible({
      timeout: 3000,
    });
    await expect(dialog.getByText('Error Message is required.')).toBeVisible({
      timeout: 3000,
    });
  });

  test('Error step saves successfully with required fields filled', async ({
    page,
  }) => {
    await page.goto(`/workflows/${workflowId}`);
    await page.waitForLoadState('networkidle');
    await expect(page.locator('.react-flow')).toBeVisible({ timeout: 15000 });

    // Add Error step
    const startNode = page
      .locator('.react-flow__node')
      .filter({ hasText: 'Start' });
    await expect(startNode).toBeVisible();
    await startNode.getByRole('button').click();

    const searchInput = page.getByPlaceholder('Search steps or operations...');
    await expect(searchInput).toBeVisible();
    await searchInput.fill('Error');

    // Click the Error step type (use description to avoid matching workflow title)
    const errorResult = page.getByText('Emit a structured error');
    await expect(errorResult).toBeVisible({ timeout: 10000 });
    await errorResult.click();

    // Fill required fields
    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible({ timeout: 5000 });

    // Fill Error Code
    await dialog
      .getByPlaceholder('Enter error code...')
      .fill('CREDIT_LIMIT_EXCEEDED');

    // Fill Error Message
    await dialog
      .getByPlaceholder('Enter error message...')
      .fill('Credit limit has been exceeded');

    // Save — should succeed and close the dialog
    await dialog.getByRole('button', { name: 'Save' }).click();

    // Verify dialog closed (save succeeded)
    await expect(dialog).not.toBeVisible({ timeout: 5000 });

    // Verify the Error node appears on the canvas
    await expect(
      page.locator('.react-flow__node').filter({ hasText: 'Error' })
    ).toBeVisible({ timeout: 10000 });
  });

  test('delete workflow', async ({ page }) => {
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
