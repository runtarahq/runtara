import { test, expect } from '@playwright/test';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

/**
 * SYN-110: Required input validation for workflow execution
 *
 * Verifies that:
 * 1. Frontend blocks execution when required fields are not set
 * 2. Empty values (empty string, empty array, 0) are accepted for required fields
 * 3. Optional fields can be cleared and are excluded from payload
 * 4. Backend rejects missing required fields
 * 5. Backend accepts empty values for required fields
 */

const __filename = fileURLToPath(import.meta.url);
const __dirnameLocal = path.dirname(__filename);

const GATEWAY_URL = process.env.GATEWAY_URL || 'http://localhost:8080';

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

test.describe
  .serial('SYN-110: Required input validation for workflow execution', () => {
  const workflowName = `SYN-110 Required Input ${Date.now()}`;
  let workflowId: string;
  let token: string;

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

  // ── Setup: create workflow with mixed required/optional input schema ──

  test('create workflow with required and optional input fields', async ({
    page,
  }) => {
    await page.goto('/workflows/create');
    await page.waitForLoadState('networkidle');
    await page.getByLabel('Name').fill(workflowName);
    await page.getByRole('button', { name: 'Save' }).click();

    await page.waitForURL(
      (url) => /\/workflows\/(?!create\b)[a-zA-Z0-9_-]+$/.test(url.pathname),
      { timeout: 15000 }
    );
    workflowId = page.url().split('/workflows/').pop()!;

    await expect(page.locator('.react-flow')).toBeVisible({ timeout: 15000 });

    // Open Settings -> Input section
    await page
      .getByRole('main')
      .getByRole('button', { name: 'Settings' })
      .click();
    await page.getByText('Input schema fields').click();

    // Add "name" field — type string, required (default)
    await page.getByRole('button', { name: 'Add Field' }).click();
    const rows = page.locator('tbody tr');
    const row1 = rows.nth(0);
    await row1.getByPlaceholder('fieldName').fill('name');
    // String type is the default

    // Add "count" field — type number, required
    await page.getByRole('button', { name: 'Add Field' }).click();
    const row2 = rows.nth(1);
    await row2.getByPlaceholder('fieldName').fill('count');
    await row2.getByRole('combobox').click();
    await page.getByRole('option', { name: 'Number' }).click();

    // Add "tags" field — type array, required
    await page.getByRole('button', { name: 'Add Field' }).click();
    const row3 = rows.nth(2);
    await row3.getByPlaceholder('fieldName').fill('tags');
    await row3.getByRole('combobox').click();
    await page.getByRole('option', { name: 'Array' }).click();

    // Add "notes" field — type string, optional
    await page.getByRole('button', { name: 'Add Field' }).click();
    const row4 = rows.nth(3);
    await row4.getByPlaceholder('fieldName').fill('notes');
    // Make it optional by unchecking Required
    await row4.getByRole('checkbox').click();

    // Add a random-double step so the workflow has a valid graph
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

    const configDialog = page.getByRole('dialog');
    await expect(configDialog).toBeVisible({ timeout: 5000 });
    await configDialog.getByRole('button', { name: 'Save' }).click();

    await expect(
      page.locator('.react-flow__node').filter({ hasText: 'Random Double' })
    ).toBeVisible({ timeout: 10000 });

    // Save via toolbar
    const saveButton = page.getByTitle('Save changes');
    await expect(saveButton).toBeVisible({ timeout: 5000 });
    await saveButton.click();
    await expect(page.getByTitle('No changes to save')).toBeVisible({
      timeout: 15000,
    });
  });

  // ── Frontend blocks unset required fields ──────────────────────────────

  test('frontend blocks execution when required fields are not set', async ({
    page,
  }) => {
    await page.goto(`/workflows/${workflowId}`);
    await page.waitForLoadState('networkidle');
    await expect(page.locator('.react-flow')).toBeVisible({ timeout: 15000 });

    const playButton = page.getByTitle('Start workflow');
    await expect(playButton).toBeVisible({ timeout: 10000 });
    await expect(playButton).toBeEnabled({ timeout: 5000 });
    await playButton.click();

    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible({ timeout: 5000 });

    // Click Execute without filling any fields
    await dialog.getByRole('button', { name: 'Execute' }).click();

    // Validation errors for required fields
    await expect(dialog.getByText('name is required')).toBeVisible();
    await expect(dialog.getByText('count is required')).toBeVisible();
    await expect(dialog.getByText('tags is required')).toBeVisible();

    // No error for optional "notes"
    await expect(dialog.getByText('notes is required')).not.toBeVisible();

    // Dialog stays open
    await expect(dialog).toBeVisible();

    await dialog.getByRole('button', { name: 'Cancel' }).click();
  });

  // ── Required fields accept empty values ────────────────────────────────

  test('required fields accept empty values (empty string, empty array, zero)', async ({
    page,
  }) => {
    await page.goto(`/workflows/${workflowId}`);
    await page.waitForLoadState('networkidle');
    await expect(page.locator('.react-flow')).toBeVisible({ timeout: 15000 });

    const playButton = page.getByTitle('Start workflow');
    await expect(playButton).toBeVisible({ timeout: 10000 });
    await expect(playButton).toBeEnabled({ timeout: 5000 });
    await playButton.click();

    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible({ timeout: 5000 });

    // Type in "name" field then clear to empty string (field is touched but empty)
    const nameInput = dialog.getByLabel('name');
    await nameInput.fill('temp');
    await nameInput.clear();

    // Enter 0 for "count"
    const countInput = dialog.getByLabel('count');
    await countInput.fill('0');

    // Click "Set value" for tags (creates empty array)
    await dialog.getByRole('button', { name: 'Set value' }).click();

    // Intercept the request to verify payload
    const executeRequestPromise = page.waitForRequest(
      (req) =>
        req.url().includes(`/workflows/${workflowId}/execute`) &&
        req.method() === 'POST'
    );

    await dialog.getByRole('button', { name: 'Execute' }).click();

    const executeRequest = await executeRequestPromise;
    const body = executeRequest.postDataJSON();
    const inputData = body?.inputs?.data;
    expect(inputData).toBeDefined();
    expect(inputData.name).toBe('');
    expect(inputData.tags).toEqual([]);
    expect(inputData.count).toBe(0);
    // notes should NOT be in payload (optional, not touched)
    expect(inputData).not.toHaveProperty('notes');
  });

  // ── Optional field clear button works ──────────────────────────────────

  test('optional field clear button removes field from payload', async ({
    page,
  }) => {
    await page.goto(`/workflows/${workflowId}`);
    await page.waitForLoadState('networkidle');
    await expect(page.locator('.react-flow')).toBeVisible({ timeout: 15000 });

    const playButton = page.getByTitle('Start workflow');
    await expect(playButton).toBeVisible({ timeout: 10000 });
    await expect(playButton).toBeEnabled({ timeout: 5000 });
    await playButton.click();

    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible({ timeout: 5000 });

    // Fill required fields
    await dialog.getByLabel('name').fill('test');
    await dialog.getByLabel('count').fill('5');
    await dialog.getByRole('button', { name: 'Set value' }).click();

    // Fill optional "notes" field
    await dialog.getByLabel('notes').fill('some notes');

    // Now clear it with the X button
    await dialog.getByTitle('Clear field').click();

    // Intercept the request
    const executeRequestPromise = page.waitForRequest(
      (req) =>
        req.url().includes(`/workflows/${workflowId}/execute`) &&
        req.method() === 'POST'
    );

    await dialog.getByRole('button', { name: 'Execute' }).click();

    const executeRequest = await executeRequestPromise;
    const body = executeRequest.postDataJSON();
    const inputData = body?.inputs?.data;
    expect(inputData).toBeDefined();
    expect(inputData.name).toBe('test');
    expect(inputData.count).toBe(5);
    // notes should NOT be in payload after clearing
    expect(inputData).not.toHaveProperty('notes');
  });

  // ── Backend rejects missing required field ─────────────────────────────

  test('backend rejects missing required field via API', async () => {
    const res = await fetch(
      `${GATEWAY_URL}/api/runtime/workflows/${workflowId}/execute`,
      {
        method: 'POST',
        headers: apiHeaders(token),
        body: JSON.stringify({
          inputs: {
            data: { notes: 'x' },
            variables: {},
          },
        }),
      }
    );

    expect(res.status).toBe(400);

    const body = await res.json();
    expect(body.success).toBe(false);
    expect(body.message).toContain('Input validation failed');
  });

  // ── Backend accepts empty values for required fields ───────────────────

  test('backend accepts empty values for required fields', async () => {
    const res = await fetch(
      `${GATEWAY_URL}/api/runtime/workflows/${workflowId}/execute`,
      {
        method: 'POST',
        headers: apiHeaders(token),
        body: JSON.stringify({
          inputs: {
            data: { name: '', count: 0, tags: [] },
            variables: {},
          },
        }),
      }
    );

    // Should not be a 400 validation error
    expect(res.status).not.toBe(400);
  });

  // ── Cleanup ────────────────────────────────────────────────────────────

  test('delete workflow', async ({ page }) => {
    await page.goto('/workflows');
    await page.waitForLoadState('networkidle');

    const card = page.locator('article').filter({ hasText: workflowName });
    await expect(card).toBeVisible({ timeout: 10000 });
    await card.getByTitle('Delete').first().click();
    await page.getByRole('button', { name: 'Confirm' }).click();
    await expect(card).not.toBeVisible({ timeout: 10000 });

    workflowId = '';
  });
});
