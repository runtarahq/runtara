import { test, expect } from '@playwright/test';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

/**
 * SYN-140: Cannot rename Conditional and Split step
 *
 * Reproduces the bug where renaming Conditional/Split steps in the
 * NodeConfigDialog did not persist to the canvas node label.
 *
 * For each step type:
 * 1. Create scenario → add step via "+" → rename in dialog → save dialog
 * 2. Verify canvas shows the custom name (not the default step type name)
 * 3. Delete scenario
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

/**
 * Helper: create scenario via UI, add a step type, rename it, and verify
 * the canvas node shows the custom name.
 *
 * Returns the scenario ID for cleanup.
 */
async function addStepAndRename(
  page: import('@playwright/test').Page,
  opts: {
    scenarioName: string;
    stepTypeName: string;
    customName: string;
  }
): Promise<string> {
  // 1. Create scenario
  await page.goto('/scenarios/create');
  await page.waitForLoadState('networkidle');
  await page.getByLabel('Name').fill(opts.scenarioName);
  await page.getByRole('button', { name: 'Save' }).click();

  await page.waitForURL(
    (url) => /\/scenarios\/(?!create\b)[a-zA-Z0-9_-]+$/.test(url.pathname),
    { timeout: 15000 }
  );
  const scenarioId = page.url().split('/scenarios/').pop()!;

  // 2. Wait for canvas
  await expect(page.locator('.react-flow')).toBeVisible({ timeout: 15000 });

  // 3. Click "+" on Start node to open StepPickerModal
  const startNode = page
    .locator('.react-flow__node')
    .filter({ hasText: 'Start' });
  await expect(startNode).toBeVisible();
  await startNode.getByRole('button').click();

  // 4. StepPickerModal opens — click the step type in browse view
  const modal = page.getByRole('dialog');
  await expect(modal).toBeVisible();
  await modal
    .getByRole('button', { name: new RegExp(`^${opts.stepTypeName}`) })
    .click();

  // 5. NodeConfigDialog opens in create mode — rename the step
  const configDialog = page.getByRole('dialog');
  await expect(configDialog).toBeVisible({ timeout: 5000 });
  const nameInput = configDialog.getByPlaceholder('Step name');
  await expect(nameInput).toBeVisible();
  await nameInput.clear();
  await nameInput.fill(opts.customName);

  // 6. Save the dialog
  await configDialog.getByRole('button', { name: 'Save' }).click();

  // 7. Verify the node shows the custom name on canvas (not the default)
  //    This is the core SYN-140 assertion — the bug caused the canvas to
  //    show the original step type name instead of the renamed value.
  await expect(
    page.locator('.react-flow__node').filter({ hasText: opts.customName })
  ).toBeVisible({ timeout: 10000 });

  return scenarioId;
}

// ── Conditional step rename ─────────────────────────────────────────

test.describe.serial('SYN-140: Conditional step rename', () => {
  const scenarioName = `SYN-140 Conditional ${Date.now()}`;
  const customName = 'My Custom Conditional';
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

  test('rename shows on canvas', async ({ page }) => {
    scenarioId = await addStepAndRename(page, {
      scenarioName,
      stepTypeName: 'Conditional',
      customName,
    });
  });

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

// ── Split step rename ───────────────────────────────────────────────

test.describe.serial('SYN-140: Split step rename', () => {
  const scenarioName = `SYN-140 Split ${Date.now()}`;
  const customName = 'My Custom Split';
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

  test('rename shows on canvas', async ({ page }) => {
    scenarioId = await addStepAndRename(page, {
      scenarioName,
      stepTypeName: 'Split',
      customName,
    });
  });

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
