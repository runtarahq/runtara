import { expect } from '@playwright/test';
import { test, buildApiKey } from '../../../fixtures';
import { SettingsPage } from '../../../pages/SettingsPage';

test.describe('Settings / API keys (mocked)', () => {
  test('renders with keys, a11y + snapshot', async ({
    page,
    mockApi,
    runA11y,
  }) => {
    await mockApi.bootstrap(page);
    await mockApi.apiKeys.list(page, [
      buildApiKey({ name: 'CI deploy key' }),
      // A null scope is what every key created before scopes existed carries, so both
      // renderings of the Scope column are covered by the snapshot.
      buildApiKey({ name: 'Read-only MCP key', scope: 'read_only' }),
      buildApiKey({ name: 'Backup key' }),
    ]);

    const view = new SettingsPage(page);
    await view.goto();

    await view.expectHeading(/api keys/i);
    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('settings-api-keys');
  });

  test('empty state', async ({ page, mockApi, runA11y }) => {
    await mockApi.bootstrap(page);
    await mockApi.apiKeys.list(page, []);

    const view = new SettingsPage(page);
    await view.goto();

    await view.expectHeading(/api keys/i);
    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('settings-api-keys-empty');
  });

  // The scope is write-once at creation — there is no edit path — so what this dialog POSTs
  // is the whole contract between it and the credential the server issues.

  test('creates an unscoped key by default', async ({ page, mockApi }) => {
    await mockApi.bootstrap(page);
    await mockApi.apiKeys.list(page, []);

    const bodies: unknown[] = [];
    await page.route(/\/api\/runtime(?:\/[^/]+)?\/api-keys$/, async (route) => {
      if (route.request().method() !== 'POST') return route.fallback();
      bodies.push(route.request().postDataJSON());
      await route.fulfill({
        status: 201,
        contentType: 'application/json',
        body: JSON.stringify({
          ...buildApiKey({ name: 'CI key' }),
          key: 'rt_org_mocked_e2e_deadbeef',
        }),
      });
    });

    const view = new SettingsPage(page);
    await view.goto();
    await page.getByRole('button', { name: /new api key/i }).click();
    await page.getByLabel('Name').fill('CI key');
    await page.getByRole('button', { name: /^create$/i }).click();

    await expect.poll(() => bodies.length).toBe(1);
    // No `scope` key at all — an unscoped key is stored exactly like a pre-scope one.
    expect(bodies[0]).toEqual({ name: 'CI key' });
  });

  test('creates a read-only key when the checkbox is ticked', async ({
    page,
    mockApi,
  }) => {
    await mockApi.bootstrap(page);
    await mockApi.apiKeys.list(page, []);

    const bodies: unknown[] = [];
    await page.route(/\/api\/runtime(?:\/[^/]+)?\/api-keys$/, async (route) => {
      if (route.request().method() !== 'POST') return route.fallback();
      bodies.push(route.request().postDataJSON());
      await route.fulfill({
        status: 201,
        contentType: 'application/json',
        body: JSON.stringify({
          ...buildApiKey({ name: 'MCP reader', scope: 'read_only' }),
          key: 'rt_org_mocked_e2e_deadbeef',
        }),
      });
    });

    const view = new SettingsPage(page);
    await view.goto();
    await page.getByRole('button', { name: /new api key/i }).click();
    await page.getByLabel('Name').fill('MCP reader');
    await page.getByLabel('Read-only').check();
    await page.getByRole('button', { name: /^create$/i }).click();

    await expect.poll(() => bodies.length).toBe(1);
    expect(bodies[0]).toEqual({ name: 'MCP reader', scope: 'read_only' });
  });
});
