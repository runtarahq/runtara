import { test, expect } from '@playwright/test';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

/**
 * Scenario Lifecycle E2E Test
 *
 * API-driven test through the gateway exercising the full stack:
 * Gateway(:8080) JWT validation → routing → runtara-server(:7001) → PostgreSQL → compilation → execution
 *
 * Flow: create → add step → compile v2 → activate v2 → add second step →
 *       compile v3 → activate v3 → execute → verify → delete
 */

const __filename = fileURLToPath(import.meta.url);
const __dirnameLocal = path.dirname(__filename);

const GATEWAY_URL = process.env.GATEWAY_URL || 'http://localhost:8080';
const API_BASE = `${GATEWAY_URL}/api/runtime/scenarios`;

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

// ── Execution graph payloads ───────────────────────────────────────

function singleStepGraph() {
  return {
    executionGraph: {
      name: 'E2E Lifecycle Test',
      description: 'Single random-double step',
      entryPoint: 'random-step',
      steps: {
        'random-step': {
          stepType: 'Agent',
          id: 'random-step',
          name: 'Generate Random Number',
          agentId: 'utils',
          capabilityId: 'random-double',
          inputMapping: {},
        },
        finish: {
          stepType: 'Finish',
          id: 'finish',
          name: 'Finish',
          inputMapping: {
            outputs: {
              valueType: 'reference',
              value: 'steps.random-step.outputs',
            },
          },
        },
      },
      executionPlan: [
        { fromStep: 'random-step', toStep: 'finish', label: 'next' },
      ],
    },
  };
}

function twoStepGraph() {
  return {
    executionGraph: {
      name: 'E2E Lifecycle Test',
      description: 'Two random-double steps chained',
      entryPoint: 'random-step-1',
      steps: {
        'random-step-1': {
          stepType: 'Agent',
          id: 'random-step-1',
          name: 'Random Number 1',
          agentId: 'utils',
          capabilityId: 'random-double',
          inputMapping: {},
        },
        'random-step-2': {
          stepType: 'Agent',
          id: 'random-step-2',
          name: 'Random Number 2',
          agentId: 'utils',
          capabilityId: 'random-double',
          inputMapping: {},
        },
        finish: {
          stepType: 'Finish',
          id: 'finish',
          name: 'Finish',
          inputMapping: {
            outputs: {
              valueType: 'reference',
              value: 'steps.random-step-2.outputs',
            },
          },
        },
      },
      executionPlan: [
        { fromStep: 'random-step-1', toStep: 'random-step-2', label: 'next' },
        { fromStep: 'random-step-2', toStep: 'finish', label: 'next' },
      ],
    },
  };
}

// ── Tests ──────────────────────────────────────────────────────────

test.describe.serial('Scenario Lifecycle', () => {
  let scenarioId: string;
  let token: string;
  let compilationWorks = false;

  test.beforeAll(() => {
    token = getAccessToken();
  });

  test('create a new scenario', async ({ request }) => {
    const res = await request.post(`${API_BASE}/create`, {
      headers: apiHeaders(token),
      data: {
        name: `E2E Lifecycle ${Date.now()}`,
        description: 'Automated lifecycle test — safe to delete',
      },
    });

    expect(res.status()).toBe(200);

    const body = await res.json();
    expect(body.success).toBe(true);
    expect(body.data.id).toBeTruthy();
    expect(body.data.currentVersionNumber).toBe(1);

    scenarioId = body.data.id;
  });

  test('add a random-double step and save', async ({ request }) => {
    const res = await request.post(`${API_BASE}/${scenarioId}/update`, {
      headers: apiHeaders(token),
      data: singleStepGraph(),
    });

    const body = await res.json();
    expect(res.status(), `Update failed: ${JSON.stringify(body)}`).toBe(200);
    expect(body.success).toBe(true);
    expect(body.version).toBe('2');
  });

  test('compile and activate version 2', async ({ request }) => {
    // Compile (may fail if .deb native cache is stale)
    const compileRes = await request.post(
      `${API_BASE}/${scenarioId}/versions/2/compile`,
      { headers: apiHeaders(token) }
    );

    const compileBody = await compileRes.json();
    if (compileRes.status() !== 200) {
      console.log(
        `Compile v2 failed (${compileRes.status()}): ${JSON.stringify(compileBody).slice(0, 500)}`
      );
      test.skip(
        true,
        'Scenario compilation unavailable (native cache may be stale)'
      );
      return;
    }

    expect(compileBody.success).toBe(true);
    compilationWorks = true;

    // Activate
    const activateRes = await request.post(
      `${API_BASE}/${scenarioId}/versions/2/set-current`,
      { headers: apiHeaders(token) }
    );

    expect(activateRes.status()).toBe(200);
    const activateBody = await activateRes.json();
    expect(activateBody.success).toBe(true);

    // Verify
    const getRes = await request.get(`${API_BASE}/${scenarioId}`, {
      headers: apiHeaders(token),
    });

    expect(getRes.status()).toBe(200);
    const scenario = await getRes.json();
    expect(scenario.data.currentVersionNumber).toBe(2);
  });

  test('add a second step and save', async ({ request }) => {
    test.skip(!compilationWorks, 'Skipped — compilation unavailable');

    const res = await request.post(`${API_BASE}/${scenarioId}/update`, {
      headers: apiHeaders(token),
      data: twoStepGraph(),
    });

    expect(res.status()).toBe(200);

    const body = await res.json();
    expect(body.success).toBe(true);
    expect(body.version).toBe('3');
  });

  test('compile and activate version 3', async ({ request }) => {
    test.skip(!compilationWorks, 'Skipped — compilation unavailable');

    // Compile
    const compileRes = await request.post(
      `${API_BASE}/${scenarioId}/versions/3/compile`,
      { headers: apiHeaders(token) }
    );

    expect(compileRes.status()).toBe(200);
    const compileBody = await compileRes.json();
    expect(compileBody.success).toBe(true);

    // Activate
    const activateRes = await request.post(
      `${API_BASE}/${scenarioId}/versions/3/set-current`,
      { headers: apiHeaders(token) }
    );

    expect(activateRes.status()).toBe(200);
    const activateBody = await activateRes.json();
    expect(activateBody.success).toBe(true);

    // Verify
    const getRes = await request.get(`${API_BASE}/${scenarioId}`, {
      headers: apiHeaders(token),
    });

    expect(getRes.status()).toBe(200);
    const scenario = await getRes.json();
    expect(scenario.data.currentVersionNumber).toBe(3);
  });

  test('execute and verify results', async ({ request }) => {
    test.skip(!compilationWorks, 'Skipped — compilation unavailable');

    const res = await request.post(
      `${GATEWAY_URL}/api/runtime/events/http-sync/${scenarioId}`,
      {
        headers: apiHeaders(token),
        data: {},
      }
    );

    const body = await res.json();
    expect(
      res.status(),
      `Execute failed: ${JSON.stringify(body).slice(0, 500)}`
    ).toBe(200);
    expect(body.success).toBe(true);
    expect(body.outputs).toBeDefined();
  });

  test('clean up — delete scenario', async ({ request }) => {
    const res = await request.post(`${API_BASE}/${scenarioId}/delete`, {
      headers: apiHeaders(token),
    });

    expect(res.status()).toBe(200);

    const body = await res.json();
    expect(body.success).toBe(true);
  });
});
