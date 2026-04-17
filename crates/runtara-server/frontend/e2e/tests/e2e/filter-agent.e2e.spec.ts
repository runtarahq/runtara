import { test, expect, APIRequestContext, Page } from '@playwright/test';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';

/**
 * Filter Agent Edge-Case E2E Tests (SYN-180)
 *
 * Tests the Filter step with various conditions and data shapes to verify
 * the filter actually filters according to the condition expression.
 *
 * Each test:
 * 1. Creates & compiles a scenario via API (hardcoded array in step config)
 * 2. Navigates to the scenario page — canvas shows the workflow
 * 3. Clicks Play → execution completes → "Completed" badge visible
 * 4. History tab shows step events (Source Array → Filter → Finish)
 * 5. Expands Filter step row to show inputs/outputs in the panel
 * 6. Verifies filtered outputs via API
 *
 * Edge cases:
 * - Simple EQ (chained step output)
 * - No matches → count: 0
 * - All items match → full pass-through
 * - Nested AND condition
 * - NOT operator
 * - Nested object property access (item.meta.category)
 * - Empty input array
 *
 * Requires: full local stack (gateway, runtime, management, frontend)
 */

const __filename = fileURLToPath(import.meta.url);
const __dirnameLocal = path.dirname(__filename);

const GATEWAY_URL = process.env.GATEWAY_URL || 'http://localhost:8080';
const API_BASE = `${GATEWAY_URL}/api/runtime/scenarios`;

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

// ── Condition builders ────────────────────────────────────────────

interface Condition {
  type: string;
  op: string;
  arguments: unknown[];
}

function eq(ref: string, value: string | number): Condition {
  return {
    type: 'operation',
    op: 'EQ',
    arguments: [
      { valueType: 'reference', value: ref },
      { valueType: 'immediate', value },
    ],
  };
}

function gt(ref: string, value: number): Condition {
  return {
    type: 'operation',
    op: 'GT',
    arguments: [
      { valueType: 'reference', value: ref },
      { valueType: 'immediate', value },
    ],
  };
}

function and(...conds: Condition[]): Condition {
  return { type: 'operation', op: 'AND', arguments: conds };
}

function not(cond: Condition): Condition {
  return { type: 'operation', op: 'NOT', arguments: [cond] };
}

// ── Graph builder ─────────────────────────────────────────────────

/**
 * Build graph: ensure-array (hardcoded items) → filter → finish.
 *
 * Items are baked into the ensure-array inputMapping as immediate values,
 * so no input schema is needed — clicking Play executes immediately.
 */
function buildFilterGraph(
  name: string,
  items: unknown[],
  condition: Condition
) {
  return {
    executionGraph: {
      name,
      description: `SYN-180 filter edge-case: ${name}`,
      entryPoint: 'ensure-step',
      steps: {
        'ensure-step': {
          stepType: 'Agent',
          id: 'ensure-step',
          name: 'Source Array',
          agentId: 'transform',
          capabilityId: 'ensure-array',
          inputMapping: {
            value: { valueType: 'immediate', value: items },
          },
        },
        'filter-step': {
          stepType: 'Filter',
          id: 'filter-step',
          name: 'Filter',
          config: {
            value: {
              valueType: 'reference',
              value: 'steps.ensure-step.outputs.items',
            },
            condition,
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
            count: {
              valueType: 'reference',
              value: 'steps.filter-step.outputs.count',
            },
          },
        },
      },
      executionPlan: [
        { fromStep: 'ensure-step', toStep: 'filter-step', label: 'next' },
        { fromStep: 'filter-step', toStep: 'finish', label: 'next' },
      ],
    },
  };
}

// ── Scenario lifecycle helpers ────────────────────────────────────

/** Create → update → compile → activate via API. Returns scenarioId. */
async function setupScenario(
  request: APIRequestContext,
  token: string,
  name: string,
  items: unknown[],
  condition: Condition
): Promise<string> {
  const createRes = await request.post(`${API_BASE}/create`, {
    headers: apiHeaders(token),
    data: { name, description: 'SYN-180 E2E — safe to delete' },
  });
  expect(createRes.status()).toBe(200);
  const scenarioId = (await createRes.json()).data.id;

  const updateRes = await request.post(`${API_BASE}/${scenarioId}/update`, {
    headers: apiHeaders(token),
    data: buildFilterGraph(name, items, condition),
  });
  expect(
    updateRes.status(),
    `Update failed: ${JSON.stringify(await updateRes.json())}`
  ).toBe(200);

  const compileRes = await request.post(
    `${API_BASE}/${scenarioId}/versions/2/compile`,
    { headers: apiHeaders(token) }
  );
  if (compileRes.status() !== 200) {
    const body = await compileRes.json();
    throw new Error(`Compile failed: ${JSON.stringify(body).slice(0, 500)}`);
  }

  const activateRes = await request.post(
    `${API_BASE}/${scenarioId}/versions/2/set-current`,
    { headers: apiHeaders(token) }
  );
  expect(activateRes.status()).toBe(200);

  return scenarioId;
}

/**
 * Execute via browser UI and verify the full visual flow:
 *   canvas → Play → Completed → History tab → expand Filter step
 *
 * Returns the execution outputs fetched via API.
 */
async function executeViaUI(
  page: Page,
  request: APIRequestContext,
  token: string,
  scenarioId: string
): Promise<Record<string, unknown>> {
  // 1. Navigate to scenario page — canvas shows the workflow
  await page.goto(`/scenarios/${scenarioId}`);
  await page.waitForLoadState('networkidle');
  await expect(page.locator('.react-flow')).toBeVisible({ timeout: 15000 });

  // 2. Click Play (no input schema → executes immediately)
  const playButton = page.getByTitle('Start scenario');
  await expect(playButton).toBeVisible({ timeout: 10000 });
  await expect(playButton).toBeEnabled({ timeout: 5000 });
  await playButton.click();

  // 3. Wait for execution to complete — clear button appears in toolbar
  const clearButton = page.getByTitle('Clear execution results');
  await expect(clearButton).toBeVisible({ timeout: 60000 });

  // 4. Click History tab in the bottom panel
  const historyTab = page.getByRole('button', { name: 'History', exact: true });
  await expect(historyTab).toBeVisible({ timeout: 5000 });
  await historyTab.click();

  // Step events table should list all 3 steps
  await expect(page.locator('text=Source Array').nth(0)).toBeVisible({
    timeout: 10000,
  });
  await expect(page.locator('text=Filter').nth(0)).toBeVisible({
    timeout: 5000,
  });
  await expect(page.locator('text=Finish').nth(0)).toBeVisible({
    timeout: 5000,
  });

  // 5. Expand the Filter step row to show its inputs/outputs
  //    Brief pause to let the events table fully render before clicking
  await page.waitForTimeout(500);
  const filterRow = page
    .locator('tr')
    .filter({
      has: page.locator('text=Filter'),
      hasNot: page.locator('text=Source Array'),
    })
    .filter({
      has: page.locator('text=Completed'),
    })
    .first();
  await filterRow.click({ timeout: 5000 });

  // Wait for expanded content to render (shows inputs/outputs JSON)
  await page.waitForTimeout(1000);

  // 6. Fetch latest instance via API for assertions
  const instancesRes = await request.get(
    `${API_BASE}/${scenarioId}/instances`,
    { headers: apiHeaders(token) }
  );
  expect(instancesRes.status()).toBe(200);
  const instancesBody = await instancesRes.json();
  const instances = instancesBody.data?.content ?? instancesBody.data ?? [];
  const latest = instances[0];
  expect(latest).toBeTruthy();

  const instanceId = latest.instanceId ?? latest.id;
  const instanceRes = await request.get(
    `${API_BASE}/${scenarioId}/instances/${instanceId}`,
    { headers: apiHeaders(token) }
  );
  expect(instanceRes.status()).toBe(200);
  const instanceBody = await instanceRes.json();

  // 7. Navigate to instance history page for detailed view
  await page.goto(`/scenarios/${scenarioId}/history/${instanceId}`);
  await page.waitForLoadState('networkidle');
  await expect(page.getByText('Scenario Execution Details')).toBeVisible({
    timeout: 15000,
  });

  // Verify output data card is visible
  await expect(page.getByText('Output Data')).toBeVisible({ timeout: 10000 });

  // Brief pause so video captures the full history page
  await page.waitForTimeout(1500);

  return (instanceBody.data?.instance?.outputs ?? {}) as Record<
    string,
    unknown
  >;
}

// ── Test data ─────────────────────────────────────────────────────

const ITEMS_MIXED = [
  { name: 'alpha', status: 'active', value: 1 },
  { name: 'beta', status: 'inactive', value: 2 },
  { name: 'alpha', status: 'active', value: 3 },
  { name: 'gamma', status: 'active', value: 4 },
];

const ITEMS_NESTED = [
  { name: 'item-1', meta: { category: 'A', priority: 1 } },
  { name: 'item-2', meta: { category: 'B', priority: 2 } },
  { name: 'item-3', meta: { category: 'A', priority: 3 } },
  { name: 'item-4', meta: { category: 'C', priority: 1 } },
];

// ── Tests (serial to avoid compilation conflicts) ─────────────────

test.describe.serial('Filter Agent Edge Cases (SYN-180)', () => {
  let token: string;
  const scenarioIds: string[] = [];

  test.beforeAll(() => {
    token = getAccessToken();
  });

  test.afterAll(async () => {
    for (const id of scenarioIds) {
      try {
        await fetch(`${API_BASE}/${id}/delete`, {
          method: 'POST',
          headers: apiHeaders(token),
        });
      } catch {
        // best-effort
      }
    }
  });

  test('simple EQ: keeps only matching items from step output', async ({
    request,
    page,
  }) => {
    const id = await setupScenario(
      request,
      token,
      `SYN-180 EQ ${Date.now()}`,
      ITEMS_MIXED,
      eq('item.name', 'alpha')
    );
    scenarioIds.push(id);

    const outputs = await executeViaUI(page, request, token, id);

    expect(outputs.count).toBe(2);
    expect(outputs.items).toHaveLength(2);
    expect(outputs.items).toEqual([
      { name: 'alpha', status: 'active', value: 1 },
      { name: 'alpha', status: 'active', value: 3 },
    ]);
  });

  test('no matches: returns empty array when nothing passes', async ({
    request,
    page,
  }) => {
    const id = await setupScenario(
      request,
      token,
      `SYN-180 NoMatch ${Date.now()}`,
      ITEMS_MIXED,
      eq('item.name', 'nonexistent')
    );
    scenarioIds.push(id);

    const outputs = await executeViaUI(page, request, token, id);

    expect(outputs.count).toBe(0);
    expect(outputs.items).toHaveLength(0);
  });

  test('all match: returns every item when all pass the condition', async ({
    request,
    page,
  }) => {
    const allActive = [
      { name: 'one', status: 'active' },
      { name: 'two', status: 'active' },
      { name: 'three', status: 'active' },
    ];

    const id = await setupScenario(
      request,
      token,
      `SYN-180 AllMatch ${Date.now()}`,
      allActive,
      eq('item.status', 'active')
    );
    scenarioIds.push(id);

    const outputs = await executeViaUI(page, request, token, id);

    expect(outputs.count).toBe(3);
    expect(outputs.items).toHaveLength(3);
    expect(outputs.items).toEqual(allActive);
  });

  test('nested AND: status EQ active AND value GT 2', async ({
    request,
    page,
  }) => {
    const id = await setupScenario(
      request,
      token,
      `SYN-180 AND ${Date.now()}`,
      ITEMS_MIXED,
      and(eq('item.status', 'active'), gt('item.value', 2))
    );
    scenarioIds.push(id);

    const outputs = await executeViaUI(page, request, token, id);

    expect(outputs.count).toBe(2);
    expect(outputs.items).toHaveLength(2);
    expect(outputs.items).toEqual([
      { name: 'alpha', status: 'active', value: 3 },
      { name: 'gamma', status: 'active', value: 4 },
    ]);
  });

  test('NOT operator: NOT(name EQ beta) keeps everything except beta', async ({
    request,
    page,
  }) => {
    const id = await setupScenario(
      request,
      token,
      `SYN-180 NOT ${Date.now()}`,
      ITEMS_MIXED,
      not(eq('item.name', 'beta'))
    );
    scenarioIds.push(id);

    const outputs = await executeViaUI(page, request, token, id);

    expect(outputs.count).toBe(3);
    expect(outputs.items).toHaveLength(3);
    expect(outputs.items).toEqual([
      { name: 'alpha', status: 'active', value: 1 },
      { name: 'alpha', status: 'active', value: 3 },
      { name: 'gamma', status: 'active', value: 4 },
    ]);
  });

  test('nested property: item.meta.category EQ A', async ({
    request,
    page,
  }) => {
    const id = await setupScenario(
      request,
      token,
      `SYN-180 Nested ${Date.now()}`,
      ITEMS_NESTED,
      eq('item.meta.category', 'A')
    );
    scenarioIds.push(id);

    const outputs = await executeViaUI(page, request, token, id);

    expect(outputs.count).toBe(2);
    expect(outputs.items).toHaveLength(2);
    expect(outputs.items).toEqual([
      { name: 'item-1', meta: { category: 'A', priority: 1 } },
      { name: 'item-3', meta: { category: 'A', priority: 3 } },
    ]);
  });

  test('empty input array: returns count 0 gracefully', async ({
    request,
    page,
  }) => {
    const id = await setupScenario(
      request,
      token,
      `SYN-180 Empty ${Date.now()}`,
      [],
      eq('item.name', 'anything')
    );
    scenarioIds.push(id);

    const outputs = await executeViaUI(page, request, token, id);

    expect(outputs.count).toBe(0);
    expect(outputs.items).toHaveLength(0);
  });
});
