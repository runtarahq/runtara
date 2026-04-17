import { test, expect } from '@playwright/test';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

/**
 * HTTP Agent Connection Dropdown E2E Test
 *
 * Verifies that bearer token connections appear in the connection picker
 * when configuring an HTTP agent step in the workflow editor.
 *
 * Flow:
 * 1. Create an http_bearer connection via API
 * 2. Create a scenario via API
 * 3. Navigate to scenario editor, add an HTTP step via UI
 * 4. Click "Select connection" → ConnectionPickerModal opens
 * 5. Verify the bearer connection is listed in the modal
 * 6. Clean up
 *
 * Requires: full local stack running (gateway, runtime, management, frontend)
 */

const __filename = fileURLToPath(import.meta.url);
const __dirnameLocal = path.dirname(__filename);

const GATEWAY_URL = process.env.GATEWAY_URL || 'http://localhost:8080';
const RUNTIME_API = `${GATEWAY_URL}/api/runtime`;

// ── Auth helpers ──────────────────────────────────────────────────

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

// ── Test ──────────────────────────────────────────────────────────

test.describe.serial('HTTP Agent Connection Dropdown', () => {
  let token: string;
  let connectionId: string;
  let scenarioId: string;

  const CONNECTION_TITLE = `E2E Bearer ${Date.now()}`;

  test.beforeAll(() => {
    token = getAccessToken();
  });

  // Safety-net cleanup
  test.afterAll(async () => {
    const headers = apiHeaders(token);

    if (scenarioId) {
      try {
        await fetch(`${RUNTIME_API}/scenarios/${scenarioId}/delete`, {
          method: 'POST',
          headers,
        });
      } catch {
        // best-effort
      }
    }

    if (connectionId) {
      try {
        await fetch(`${RUNTIME_API}/connections/${connectionId}`, {
          method: 'DELETE',
          headers,
        });
      } catch {
        // best-effort
      }
    }
  });

  test('create bearer connection and scenario via API', async ({ request }) => {
    const headers = apiHeaders(token);

    // 1. Create an http_bearer connection
    const connRes = await request.post(`${RUNTIME_API}/connections`, {
      headers,
      data: {
        title: CONNECTION_TITLE,
        integrationId: 'http_bearer',
        connectionParameters: {
          token: 'test-e2e-token-12345',
        },
      },
    });
    expect(
      connRes.status(),
      `Create connection failed: ${await connRes.text()}`
    ).toBe(201);
    const connBody = await connRes.json();
    connectionId = connBody.connectionId;
    expect(connectionId).toBeTruthy();

    // 2. Create a scenario
    const scenarioRes = await request.post(`${RUNTIME_API}/scenarios/create`, {
      headers,
      data: {
        name: `E2E HTTP Dropdown ${Date.now()}`,
        description: 'E2E test — safe to delete',
      },
    });
    expect(scenarioRes.status()).toBe(200);
    const scenarioBody = await scenarioRes.json();
    scenarioId = scenarioBody.data?.id;
    expect(scenarioId).toBeTruthy();
  });

  test('connection picker shows bearer connection for HTTP step', async ({
    page,
  }) => {
    // 1. Navigate to scenario editor
    await page.goto(`/scenarios/${scenarioId}`);
    await page.waitForLoadState('networkidle');
    await expect(page.locator('.react-flow')).toBeVisible({ timeout: 15000 });

    // 2. Click "+" on Start node to add a step
    const startNode = page
      .locator('.react-flow__node')
      .filter({ hasText: 'Start' });
    await expect(startNode).toBeVisible();
    await startNode.getByRole('button').click();

    // 3. Search for HTTP Request in the step picker
    const searchInput = page.getByPlaceholder('Search steps or operations...');
    await expect(searchInput).toBeVisible();
    await searchInput.fill('HTTP Request');

    // 4. Select the HTTP Request capability
    const capabilityResult = page.getByText('HTTP Request').first();
    await expect(capabilityResult).toBeVisible({ timeout: 30000 });
    await capabilityResult.click();

    // 5. NodeConfigDialog opens with "Select connection" button in the header
    const dialog = page.getByRole('dialog');
    await expect(dialog).toBeVisible({ timeout: 5000 });

    // The "Select connection" button is in the subtitle area
    const selectConnectionBtn = dialog.getByRole('button', {
      name: 'Select connection',
    });
    await expect(selectConnectionBtn).toBeVisible({ timeout: 10000 });

    // 6. Click "Select connection" to open the ConnectionPickerModal
    await selectConnectionBtn.click();

    // 7. ConnectionPickerModal opens as a second dialog with title "Select Connection"
    const pickerDialog = page.getByRole('dialog', {
      name: 'Select Connection',
    });
    await expect(pickerDialog).toBeVisible({ timeout: 5000 });

    // 8. Verify "None (Manual auth)" option is present
    await expect(pickerDialog.getByText('None (Manual auth)')).toBeVisible();

    // 9. Verify our bearer connection appears in the list
    await expect(pickerDialog.getByText(CONNECTION_TITLE)).toBeVisible({
      timeout: 5000,
    });
  });
});
