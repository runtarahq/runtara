import { test, expect } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { appPath } from '../utils/app-path';

const fixtureFile = process.env.E2E_OPERATIONS_FIXTURE;
test('Runs shares date bounds, keeps URL context and recovers from refresh failures', async ({
  page,
}) => {
  test.skip(
    !fixtureFile,
    'Set E2E_OPERATIONS_FIXTURE to an isolated live fixture'
  );
  const fixture = JSON.parse(readFileSync(fixtureFile!, 'utf8')) as {
    failureWorkflow: string;
    failureLabel: string;
  };
  const calls: { path: string; data: Record<string, unknown> }[] = [];
  page.on('request', (request) => {
    const url = new URL(request.url());
    if (url.pathname.endsWith('/executions/summary'))
      calls.push({ path: 'summary', data: request.postDataJSON() });
    if (url.pathname.endsWith('/executions'))
      calls.push({ path: 'list', data: Object.fromEntries(url.searchParams) });
  });
  await page.goto(
    appPath(
      `/operations/monitor?workflowId=${fixture.failureWorkflow}&runLabel=${fixture.failureLabel}&status=failed,timeout&range=24h&dateBasis=completed&sortBy=completedAt&sortOrder=asc#results`
    )
  );
  await expect(page).toHaveURL(/\/operations\/runs\?.*#results$/);
  await expect(
    page.getByRole('heading', { name: 'Runs', exact: true })
  ).toBeVisible();
  await expect(
    page.getByRole('button', { name: /^Failed \d/ })
  ).toHaveAttribute('aria-pressed', 'true');
  await page.waitForLoadState('networkidle');
  expect(calls.filter((call) => call.path === 'summary')).toHaveLength(1);
  expect(calls.filter((call) => call.path === 'list')).toHaveLength(1);
  const summary = calls.find((call) => call.path === 'summary')!.data;
  const listing = calls.find((call) => call.path === 'list')!.data;
  expect(summary.completedFrom).toBe(listing.completedFrom);
  expect(summary.completedTo).toBe(listing.completedTo);
  expect(summary).not.toHaveProperty('status');
  expect(summary.workflowId).toBe(fixture.failureWorkflow);
  await page.getByRole('switch', { name: 'Refresh every 30 s' }).uncheck();
  const row = page
    .getByRole('row')
    .filter({
      has: page.getByRole('link', { name: fixture.failureLabel, exact: true }),
    })
    .first();
  await expect(row).toBeVisible();
  await page.getByRole('button', { name: /^All \d/ }).click();
  await expect(page.getByLabel('Date basis')).toHaveValue('completed');
  await expect(page.getByLabel('Run time range')).toHaveValue('24h');
  await expect(page.getByLabel('Run order')).toHaveValue('completedAt:asc');
  await page.goBack();
  await expect(
    page.getByRole('button', { name: /^Failed \d/ })
  ).toHaveAttribute('aria-pressed', 'true');
  await page.reload();
  await expect(page.getByLabel('Run order')).toHaveValue('completedAt:asc');
  await expect(row).toBeVisible();

  await row.getByText('Error details', { exact: true }).click();
  await expect(row.locator('details')).toHaveAttribute('open', '');
  await expect(page.getByRole('status')).toContainText('Auto-refresh paused');

  await page.route('**/api/runtime/executions?*', (route) =>
    route.fulfill({
      status: 500,
      contentType: 'application/json',
      body: '{"error":"Test refresh failure"}',
    })
  );
  await page.getByRole('button', { name: 'Refresh', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('Showing stale data');
  await expect(row).toBeVisible();
  await expect(page.getByRole('button', { name: 'Failed —' })).toBeVisible();
  await page.unroute('**/api/runtime/executions?*');
  await page.route('**/api/runtime/executions/summary', (route) =>
    route.fulfill({
      status: 500,
      contentType: 'application/json',
      body: '{"error":"Test counts failure"}',
    })
  );
  await page.getByRole('button', { name: 'Refresh', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText(
    'Status counts unavailable'
  );
  await expect(row).toBeVisible();
  await page.unroute('**/api/runtime/executions/summary');
  await page.getByRole('button', { name: 'Refresh', exact: true }).click();
  await expect(page.getByRole('alert')).toHaveCount(0);
  await expect(page.getByRole('button', { name: /^Failed \d/ })).toBeVisible();
});
