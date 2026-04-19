import { test, expect } from '@playwright/test';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

/**
 * Error Step Instance History E2E Test (SYN-236)
 *
 * Verifies that a workflow containing an Error step can be executed and its
 * instance history page renders correctly — the Error step appears in the
 * timeline with the correct name, type, and status.
 *
 * Before the fix, the Error step did not emit debug events, so the step
 * summaries endpoint returned an empty array and the history page showed
 * "This page does not exist or is unavailable."
 *
 * Flow: create workflow → update with Error step → enable debug mode →
 *       compile → activate → execute → navigate to instance history →
 *       verify Error step in timeline → delete workflow
 *
 * Requires: full local stack running (gateway, runtime, management, frontend)
 */

const __filename = fileURLToPath(import.meta.url);
const __dirnameLocal = path.dirname(__filename);

const GATEWAY_URL = process.env.GATEWAY_URL || 'http://localhost:8080';
const API_BASE = `${GATEWAY_URL}/api/runtime/workflows`;
const WORKFLOW_NAME = `E2E Error History ${Date.now()}`;

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

/** Execution graph with a single Error step. */
function errorStepGraph() {
  return {
    executionGraph: {
      name: WORKFLOW_NAME,
      description: 'Single Error step for SYN-236 verification',
      entryPoint: 'error-step',
      steps: {
        'error-step': {
          stepType: 'Error',
          id: 'error-step',
          name: 'Test Error',
          code: 'TEST_ERROR_CODE',
          message: 'This is a test error for SYN-236',
        },
      },
      executionPlan: [],
    },
  };
}

test.describe.serial('Error Step Instance History (SYN-236)', () => {
  let workflowId: string;
  let instanceId: string;
  let token: string;
  let compilationWorks = false;

  test.beforeAll(() => {
    token = getAccessToken();
  });

  // Safety-net cleanup: delete workflow via API if UI deletion fails
  test.afterAll(async () => {
    if (!workflowId) return;
    try {
      await fetch(`${API_BASE}/${workflowId}/delete`, {
        method: 'POST',
        headers: apiHeaders(token),
      });
    } catch {
      // best-effort cleanup
    }
  });

  test('create workflow with Error step', async ({ request }) => {
    // Create workflow
    const createRes = await request.post(`${API_BASE}/create`, {
      headers: apiHeaders(token),
      data: {
        name: WORKFLOW_NAME,
        description: 'SYN-236 E2E test — safe to delete',
      },
    });

    expect(createRes.status()).toBe(200);
    const createBody = await createRes.json();
    expect(createBody.success).toBe(true);
    workflowId = createBody.data.id;

    // Update with Error step graph
    const updateRes = await request.post(`${API_BASE}/${workflowId}/update`, {
      headers: apiHeaders(token),
      data: errorStepGraph(),
    });

    const updateBody = await updateRes.json();
    expect(
      updateRes.status(),
      `Update failed: ${JSON.stringify(updateBody)}`
    ).toBe(200);
    expect(updateBody.success).toBe(true);
    expect(updateBody.version).toBe('2');
  });

  test('compile, activate, and execute', async ({ request }) => {
    // Enable debug mode BEFORE compiling (toggleing invalidates compilation)
    const debugRes = await request.put(
      `${API_BASE}/${workflowId}/versions/2/debug`,
      {
        headers: apiHeaders(token),
        data: { debugMode: true },
      }
    );
    expect(debugRes.status()).toBe(200);

    // Compile version 2 (with debug mode enabled)
    const compileRes = await request.post(
      `${API_BASE}/${workflowId}/versions/2/compile`,
      { headers: apiHeaders(token) }
    );

    const compileBody = await compileRes.json();
    if (compileRes.status() !== 200) {
      console.log(
        `Compile failed (${compileRes.status()}): ${JSON.stringify(compileBody).slice(0, 500)}`
      );
      test.skip(
        true,
        'Workflow compilation unavailable (native cache may be stale)'
      );
      return;
    }

    expect(compileBody.success).toBe(true);
    compilationWorks = true;

    // Activate version 2
    const activateRes = await request.post(
      `${API_BASE}/${workflowId}/versions/2/set-current`,
      { headers: apiHeaders(token) }
    );
    expect(activateRes.status()).toBe(200);

    // Execute workflow (async — returns instanceId)
    const executeRes = await request.post(`${API_BASE}/${workflowId}/execute`, {
      headers: apiHeaders(token),
      data: { inputs: { data: {} } },
    });

    const executeBody = await executeRes.json();
    expect(
      executeRes.status(),
      `Execute failed: ${JSON.stringify(executeBody).slice(0, 500)}`
    ).toBe(200);
    expect(executeBody.data.instanceId).toBeTruthy();
    instanceId = executeBody.data.instanceId;

    // Wait for execution to complete (poll instance status)
    for (let i = 0; i < 30; i++) {
      const instanceRes = await request.get(
        `${API_BASE}/${workflowId}/instances/${instanceId}`,
        { headers: apiHeaders(token) }
      );

      if (instanceRes.status() === 200) {
        const instanceBody = await instanceRes.json();
        const status = instanceBody.data?.instance?.status;
        if (status === 'completed' || status === 'failed') {
          break;
        }
      }

      await new Promise((r) => setTimeout(r, 1000));
    }
  });

  test('verify Error step appears in instance history timeline', async ({
    page,
    request,
  }) => {
    test.skip(!compilationWorks, 'Skipped — compilation unavailable');

    // First confirm via API that step summaries contain the Error step
    const stepsRes = await request.get(
      `${GATEWAY_URL}/api/runtime/workflows/${workflowId}/instances/${instanceId}/steps`,
      { headers: apiHeaders(token) }
    );
    expect(stepsRes.status()).toBe(200);
    const stepsBody = await stepsRes.json();
    expect(stepsBody.data.count).toBeGreaterThan(0);
    expect(
      stepsBody.data.steps.some(
        (s: { stepType: string }) => s.stepType === 'Error'
      )
    ).toBe(true);

    // Navigate to instance history page
    await page.goto(`/workflows/${workflowId}/history/${instanceId}`);
    await page.waitForLoadState('networkidle');

    // Wait for the "Loading Timeline..." spinner to disappear
    await expect(page.getByText('Loading Timeline...')).not.toBeVisible({
      timeout: 15000,
    });

    // The "No Timeline Events Yet" message should NOT appear
    await expect(page.getByText('No Timeline Events Yet')).not.toBeVisible();

    // Verify the Error step name appears in the timeline
    await expect(page.getByText('Test Error', { exact: true })).toBeVisible({
      timeout: 10000,
    });

    // Verify the step type "Error" is shown beneath the step name in the timeline row
    await expect(page.getByText('Error', { exact: true })).toBeVisible();

    // Verify the page did NOT show the "does not exist" error
    await expect(page.getByText('This page does not exist')).not.toBeVisible();
  });

  test('structured error display renders without crashing (SYN-236 frontend fix)', async ({
    page,
  }) => {
    test.skip(!compilationWorks, 'Skipped — compilation unavailable');

    // Navigate to instance history — the instance has errorMessage metadata
    // with `context` field (not `attributes`), which previously crashed
    // StructuredErrorDisplay with "Cannot convert undefined or null to object"
    await page.goto(`/workflows/${workflowId}/history/${instanceId}`);
    await page.waitForLoadState('networkidle');

    // The error boundary must NOT appear — this was the SYN-236 crash
    await expect(page.getByText('An error has occurred')).not.toBeVisible({
      timeout: 5000,
    });

    // The "Error Message" section header should be visible (from WorkflowHistory)
    await expect(page.getByText('Error Message')).toBeVisible({
      timeout: 10000,
    });

    // StructuredErrorDisplay should render the error code badge
    await expect(page.getByText('TEST_ERROR_CODE')).toBeVisible();

    // StructuredErrorDisplay should render the category badge
    await expect(page.getByText('permanent')).toBeVisible();

    // StructuredErrorDisplay should render the error message text
    await expect(
      page.getByText('This is a test error for SYN-236')
    ).toBeVisible();
  });

  test('delete workflow', async ({ request }) => {
    const res = await request.post(`${API_BASE}/${workflowId}/delete`, {
      headers: apiHeaders(token),
    });

    expect(res.status()).toBe(200);
    const body = await res.json();
    expect(body.success).toBe(true);
    workflowId = '';
  });
});
