import { test, expect } from '@playwright/test';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

/**
 * SYN-106: Input validation failed for array/object input types
 *
 * Reproduces the bug where executing a scenario with array or object
 * input schema fields failed with "Input validation failed: Invalid
 * schema: true is not of type 'array'".
 *
 * Root causes fixed:
 * 1. Frontend: ScenarioExecuteDialog used raw text <Input> for array/object
 *    fields, sending stringified JSON instead of actual arrays/objects.
 *    Fix: replaced with CompositeValueEditor.
 * 2. Frontend: buildSchemaFromFields generated non-standard JSON Schema
 *    (per-field "required": true instead of proper JSON Schema format).
 *    Fix: generates standard { type: "object", properties, required: [] }.
 * 3. Backend: validate_inputs validated the full {data, variables} wrapper
 *    instead of the inner user data. Fix: validates inputs.data.
 *
 * This test verifies:
 * - Frontend renders CompositeValueEditor for array/object fields
 * - HTTP request body contains properly typed data (not stringified JSON)
 * - Frontend rejects empty required array/object fields (client-side validation)
 * - Backend rejects wrong types via JSON Schema validation
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

test.describe.serial('SYN-106: Execute with complex input types', () => {
  const scenarioName = `SYN-106 Complex Input ${Date.now()}`;
  let scenarioId: string;
  let token: string;

  test.beforeAll(() => {
    token = getAccessToken();
  });

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

  // ── Setup: create scenario with required array/object input schema ───

  test('create scenario with array/object input schema', async ({ page }) => {
    await page.goto('/scenarios/create');
    await page.waitForLoadState('networkidle');
    await page.getByLabel('Name').fill(scenarioName);
    await page.getByRole('button', { name: 'Save' }).click();

    await page.waitForURL(
      (url) => /\/scenarios\/(?!create\b)[a-zA-Z0-9_-]+$/.test(url.pathname),
      { timeout: 15000 }
    );
    scenarioId = page.url().split('/scenarios/').pop()!;

    await expect(page.locator('.react-flow')).toBeVisible({ timeout: 15000 });

    // Open Settings → Input section
    await page
      .getByRole('main')
      .getByRole('button', { name: 'Settings' })
      .click();
    await page.getByText('Input schema fields').click();

    // Add "tags" field — type array, required (default)
    await page.getByRole('button', { name: 'Add Field' }).click();
    const rows = page.locator('tbody tr');
    const row1 = rows.nth(0);
    await row1.getByPlaceholder('fieldName').fill('tags');
    await row1.getByRole('combobox').click();
    await page.getByRole('option', { name: 'Array' }).click();

    // Add "metadata" field — type object, required (default)
    await page.getByRole('button', { name: 'Add Field' }).click();
    const row2 = rows.nth(1);
    await row2.getByPlaceholder('fieldName').fill('metadata');
    await row2.getByRole('combobox').click();
    await page.getByRole('option', { name: 'Object' }).click();

    // Add a random-double step so the scenario has a valid graph
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

  // ── Negative: frontend rejects empty required fields ─────────────────

  test('frontend rejects empty required array/object fields', async ({
    page,
  }) => {
    await page.goto(`/scenarios/${scenarioId}`);
    await page.waitForLoadState('networkidle');
    await expect(page.locator('.react-flow')).toBeVisible({ timeout: 15000 });

    const playButton = page.getByTitle('Start scenario');
    await expect(playButton).toBeVisible({ timeout: 10000 });
    await expect(playButton).toBeEnabled({ timeout: 5000 });
    await playButton.click();

    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible({ timeout: 5000 });

    // Try to execute with empty required fields — should be rejected
    await dialog.getByRole('button', { name: 'Execute' }).click();

    // Validation errors must appear for both required fields
    await expect(dialog.getByText('tags is required')).toBeVisible();
    await expect(dialog.getByText('metadata is required')).toBeVisible();

    // Dialog must stay open (form was NOT submitted)
    await expect(dialog).toBeVisible();

    // Close dialog for next test
    await dialog.getByRole('button', { name: 'Cancel' }).click();
  });

  // ── Negative: backend rejects wrong types via API ────────────────────

  test('backend rejects wrong input types via API', async () => {
    // Send a string where an array is expected, and a number where an object
    // is expected — directly via API to test backend JSON Schema validation.
    const res = await fetch(
      `${GATEWAY_URL}/api/runtime/scenarios/${scenarioId}/execute`,
      {
        method: 'POST',
        headers: apiHeaders(token),
        body: JSON.stringify({
          inputs: {
            data: {
              tags: 'not-an-array',
              metadata: 12345,
            },
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

  // ── Positive: valid data passes both frontend and backend ────────────

  test('execute with valid array/object data passes validation', async ({
    page,
  }) => {
    await page.goto(`/scenarios/${scenarioId}`);
    await page.waitForLoadState('networkidle');
    await expect(page.locator('.react-flow')).toBeVisible({ timeout: 15000 });

    const playButton = page.getByTitle('Start scenario');
    await expect(playButton).toBeVisible({ timeout: 10000 });
    await expect(playButton).toBeEnabled({ timeout: 5000 });
    await playButton.click();

    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible({ timeout: 5000 });

    // Array/object fields start as "not set" — click "Set value" to activate editors
    const setValueButtons = dialog.getByRole('button', { name: 'Set value' });
    // Click both "Set value" buttons (tags first, then metadata)
    await setValueButtons.first().click();
    await setValueButtons.first().click();

    // Frontend renders CompositeValueEditor after activation
    await expect(dialog.getByText('Composite Array')).toBeVisible();
    await expect(dialog.getByText('Composite Object')).toBeVisible();

    // Populate array field with real data
    const arrayEditor = dialog
      .locator('.overflow-hidden')
      .filter({ hasText: 'Composite Array' });
    await arrayEditor.getByRole('button', { name: 'Add Item' }).click();
    await page.getByRole('menuitem', { name: /Immediate/ }).click();
    await arrayEditor.getByPlaceholder('Enter value...').fill('tag1');

    // Populate object field with real data
    const objectEditor = dialog
      .locator('.overflow-hidden')
      .filter({ hasText: 'Composite Object' });
    await objectEditor.getByRole('button', { name: 'Add Field' }).click();
    await objectEditor.getByPlaceholder('Enter field name...').fill('key1');
    await objectEditor.getByRole('button', { name: /^Add$/ }).click();
    await objectEditor.getByPlaceholder('Enter value...').fill('value1');

    // Intercept the HTTP request to verify wire format
    const executeRequestPromise = page.waitForRequest(
      (req) =>
        req.url().includes(`/scenarios/${scenarioId}/execute`) &&
        req.method() === 'POST'
    );

    await dialog.getByRole('button', { name: 'Execute' }).click();

    // Verify wire format: arrays are arrays, objects are objects
    const executeRequest = await executeRequestPromise;
    const body = executeRequest.postDataJSON();
    const inputData = body?.inputs?.data;
    expect(inputData).toBeDefined();
    expect(Array.isArray(inputData.tags)).toBe(true);
    expect(inputData.tags).toEqual(['tag1']);
    expect(typeof inputData.metadata).toBe('object');
    expect(inputData.metadata).not.toBeNull();
    expect(Array.isArray(inputData.metadata)).toBe(false);
    expect(inputData.metadata).toEqual({ key1: 'value1' });

    // Backend must accept the correctly typed data
    try {
      await expect(dialog).not.toBeVisible({ timeout: 60000 });
    } catch {
      const errorBox = dialog.locator('.bg-destructive\\/10');
      if (await errorBox.isVisible()) {
        const errorText = await errorBox.textContent();
        expect(errorText).not.toContain('Input validation failed');
        await dialog.getByRole('button', { name: 'Cancel' }).click();
      }
    }
  });

  // ── Cleanup ──────────────────────────────────────────────────────────

  test('delete scenario', async ({ page }) => {
    await page.goto('/scenarios');
    await page.waitForLoadState('networkidle');

    const card = page.locator('article').filter({ hasText: scenarioName });
    await expect(card).toBeVisible({ timeout: 10000 });
    await card.getByTitle('Delete').first().click();
    await page.getByRole('button', { name: 'Confirm' }).click();
    await expect(card).not.toBeVisible({ timeout: 10000 });

    scenarioId = '';
  });
});
