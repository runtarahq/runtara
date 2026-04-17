import { test, expect } from '@playwright/test';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

/**
 * Step Configuration Save E2E Tests (SYN-217, SYN-244, SYN-242)
 *
 * Verifies that Filter, Switch (with Default), and Log step configurations
 * save correctly via the runtime API and persist after reload.
 *
 * SYN-217: Filter step condition wasn't saving (filterCondition stripped by Zod)
 * SYN-244: Switch step with Default output wasn't saving (config dropped when value empty)
 * SYN-242: Log step couldn't be configured (missing frontend handler)
 *
 * Uses runtime API directly (X-Org-Id header) for scenario CRUD.
 *
 * Requires: smo-runtime on :7001 with PostgreSQL
 */

const __filename = fileURLToPath(import.meta.url);
const __dirnameLocal = path.dirname(__filename);

const RUNTIME_URL = process.env.RUNTIME_URL || 'http://localhost:7001';
const API_BASE = `${RUNTIME_URL}/api/runtime/scenarios`;
const TENANT_ID = process.env.TEST_ORG_ID || 'org_xxxxx';

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
    'X-Org-Id': TENANT_ID,
  };
}

// ---------- SYN-217: Filter step condition saves ----------

test.describe.serial('SYN-217: Filter step condition saves', () => {
  let scenarioId: string;
  let token: string;

  test.beforeAll(() => {
    token = getAccessToken();
  });

  test.afterAll(async () => {
    if (!scenarioId) return;
    try {
      await fetch(`${API_BASE}/${scenarioId}/delete`, {
        method: 'POST',
        headers: apiHeaders(token),
      });
    } catch {
      // best-effort cleanup
    }
  });

  test('save Filter step with condition and verify it persists', async ({
    request,
  }) => {
    // 1. Create scenario
    const createRes = await request.post(`${API_BASE}/create`, {
      headers: apiHeaders(token),
      data: {
        name: `SYN-217 Filter Save ${Date.now()}`,
        description: 'SYN-217 E2E — safe to delete',
      },
    });
    expect(createRes.status()).toBe(200);
    scenarioId = (await createRes.json()).data.id;

    // 2. Update with a Filter step that has a condition
    const filterCondition = {
      type: 'operation',
      op: 'EQ',
      arguments: [
        { valueType: 'reference', value: 'item.status' },
        { valueType: 'immediate', value: 'active' },
      ],
    };

    const updateRes = await request.post(`${API_BASE}/${scenarioId}/update`, {
      headers: apiHeaders(token),
      data: {
        executionGraph: {
          name: 'SYN-217 Filter Save',
          entryPoint: 'source-step',
          steps: {
            'source-step': {
              stepType: 'Agent',
              id: 'source-step',
              name: 'Source',
              agentId: 'transform',
              capabilityId: 'ensure-array',
              inputMapping: {
                value: {
                  valueType: 'immediate',
                  value: [{ status: 'active' }, { status: 'inactive' }],
                },
              },
            },
            'filter-step': {
              stepType: 'Filter',
              id: 'filter-step',
              name: 'Filter',
              config: {
                value: {
                  valueType: 'reference',
                  value: 'steps.source-step.outputs.items',
                },
                condition: filterCondition,
              },
            },
            finish: {
              stepType: 'Finish',
              id: 'finish',
              name: 'Finish',
              inputMapping: {
                items: {
                  valueType: 'reference',
                  value: 'steps.filter-step.outputs.items',
                },
              },
            },
          },
          executionPlan: [
            {
              fromStep: 'source-step',
              toStep: 'filter-step',
              label: 'next',
            },
            { fromStep: 'filter-step', toStep: 'finish', label: 'next' },
          ],
        },
      },
    });
    const updateBody = await updateRes.json();
    expect(
      updateRes.status(),
      `Update failed: ${JSON.stringify(updateBody).slice(0, 500)}`
    ).toBe(200);

    // 3. Fetch scenario back and verify filter config persists
    const getRes = await request.get(`${API_BASE}/${scenarioId}`, {
      headers: apiHeaders(token),
    });
    expect(getRes.status()).toBe(200);
    const scenario = await getRes.json();
    const filterStep = scenario.data.executionGraph.steps['filter-step'];

    expect(filterStep).toBeTruthy();
    expect(filterStep.config).toBeTruthy();
    expect(filterStep.config.condition).toBeTruthy();
    expect(filterStep.config.condition.op).toBe('EQ');
    expect(filterStep.config.value.value).toBe(
      'steps.source-step.outputs.items'
    );
  });
});

// ---------- SYN-244: Switch step with Default output saves ----------

test.describe.serial('SYN-244: Switch step with Default output saves', () => {
  let scenarioId: string;
  let token: string;

  test.beforeAll(() => {
    token = getAccessToken();
  });

  test.afterAll(async () => {
    if (!scenarioId) return;
    try {
      await fetch(`${API_BASE}/${scenarioId}/delete`, {
        method: 'POST',
        headers: apiHeaders(token),
      });
    } catch {
      // best-effort cleanup
    }
  });

  test('save Switch step with default output and verify it persists', async ({
    request,
  }) => {
    // 1. Create scenario
    const createRes = await request.post(`${API_BASE}/create`, {
      headers: apiHeaders(token),
      data: {
        name: `SYN-244 Switch Default ${Date.now()}`,
        description: 'SYN-244 E2E — safe to delete',
      },
    });
    expect(createRes.status()).toBe(200);
    scenarioId = (await createRes.json()).data.id;

    // 2. Update with a Switch step that has cases and a default output
    const updateRes = await request.post(`${API_BASE}/${scenarioId}/update`, {
      headers: apiHeaders(token),
      data: {
        executionGraph: {
          name: 'SYN-244 Switch Default',
          entryPoint: 'source-step',
          steps: {
            'source-step': {
              stepType: 'Agent',
              id: 'source-step',
              name: 'Source',
              agentId: 'utils',
              capabilityId: 'random-double',
              inputMapping: {},
            },
            'switch-step': {
              stepType: 'Switch',
              id: 'switch-step',
              name: 'Switch',
              config: {
                value: {
                  valueType: 'immediate',
                  value: 'active',
                },
                cases: [{ match: 'active', matchType: 'EQ', output: 'Active' }],
                default: { output: 'Default' },
              },
            },
            finish: {
              stepType: 'Finish',
              id: 'finish',
              name: 'Finish',
              inputMapping: {},
            },
          },
          executionPlan: [
            { fromStep: 'source-step', toStep: 'switch-step', label: 'next' },
            { fromStep: 'switch-step', toStep: 'finish', label: 'next' },
          ],
        },
      },
    });
    const updateBody = await updateRes.json();
    expect(
      updateRes.status(),
      `Update failed: ${JSON.stringify(updateBody).slice(0, 500)}`
    ).toBe(200);

    // 3. Verify switch config persists with default
    const getRes = await request.get(`${API_BASE}/${scenarioId}`, {
      headers: apiHeaders(token),
    });
    expect(getRes.status()).toBe(200);
    const scenario = await getRes.json();
    const switchStep = scenario.data.executionGraph.steps['switch-step'];

    expect(switchStep).toBeTruthy();
    expect(switchStep.config).toBeTruthy();
    expect(switchStep.config.default).toBeTruthy();
    expect(switchStep.config.cases).toHaveLength(1);
    expect(switchStep.config.cases[0].match).toBe('active');
  });
});

// ---------- SYN-242: Log step saves and loads correctly ----------

test.describe.serial('SYN-242: Log step saves and loads correctly', () => {
  let scenarioId: string;
  let token: string;

  test.beforeAll(() => {
    token = getAccessToken();
  });

  test.afterAll(async () => {
    if (!scenarioId) return;
    try {
      await fetch(`${API_BASE}/${scenarioId}/delete`, {
        method: 'POST',
        headers: apiHeaders(token),
      });
    } catch {
      // best-effort cleanup
    }
  });

  test('save Log step via API and verify roundtrip', async ({ request }) => {
    // 1. Create scenario
    const createRes = await request.post(`${API_BASE}/create`, {
      headers: apiHeaders(token),
      data: {
        name: `SYN-242 Log Step ${Date.now()}`,
        description: 'SYN-242 E2E — safe to delete',
      },
    });
    expect(createRes.status()).toBe(200);
    scenarioId = (await createRes.json()).data.id;

    // 2. Update with a Log step
    const updateRes = await request.post(`${API_BASE}/${scenarioId}/update`, {
      headers: apiHeaders(token),
      data: {
        executionGraph: {
          name: 'SYN-242 Log Step',
          entryPoint: 'log-step',
          steps: {
            'log-step': {
              stepType: 'Log',
              id: 'log-step',
              name: 'Log',
              level: 'warn',
              message: 'Processing complete',
            },
            finish: {
              stepType: 'Finish',
              id: 'finish',
              name: 'Finish',
              inputMapping: {},
            },
          },
          executionPlan: [
            { fromStep: 'log-step', toStep: 'finish', label: 'next' },
          ],
        },
      },
    });
    const updateBody = await updateRes.json();
    expect(
      updateRes.status(),
      `Update failed: ${JSON.stringify(updateBody).slice(0, 500)}`
    ).toBe(200);

    // 3. Fetch back and verify Log step fields persist
    const getRes = await request.get(`${API_BASE}/${scenarioId}`, {
      headers: apiHeaders(token),
    });
    expect(getRes.status()).toBe(200);
    const scenario = await getRes.json();
    const logStep = scenario.data.executionGraph.steps['log-step'];

    expect(logStep).toBeTruthy();
    expect(logStep.stepType).toBe('Log');
    expect(logStep.message).toBe('Processing complete');
    expect(logStep.level).toBe('warn');
  });
});
