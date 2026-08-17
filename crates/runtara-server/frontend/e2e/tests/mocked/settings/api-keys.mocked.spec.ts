import { expect, type Page } from '@playwright/test';
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

  test('tells an expired key apart from an active one', async ({
    page,
    mockApi,
  }) => {
    // An expired key is never revoked — the server just stops validating it — so without an
    // expiry-aware status it would sit here labelled "Active" while refusing every request.
    const inDays = (days: number) =>
      new Date(Date.now() + days * 86_400_000).toISOString();
    await mockApi.bootstrap(page);
    await mockApi.apiKeys.list(page, [
      buildApiKey({ name: 'Long lived key', expires_at: inDays(90) }),
      buildApiKey({ name: 'Nearly done key', expires_at: inDays(3) }),
      buildApiKey({ name: 'Lapsed key', expires_at: inDays(-1) }),
      buildApiKey({ name: 'Permanent key' }),
    ]);

    const view = new SettingsPage(page);
    await view.goto();
    await view.expectHeading(/api keys/i);

    const rowFor = (name: string) =>
      page.getByRole('row').filter({ hasText: name });
    await expect(rowFor('Long lived key')).toContainText('Active');
    await expect(rowFor('Nearly done key')).toContainText('Expiring');
    await expect(rowFor('Lapsed key')).toContainText('Expired');
    await expect(rowFor('Permanent key')).toContainText('No expiration');
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

  // Scope and expiry are both write-once at creation — there is no edit path — so what this
  // dialog POSTs is the whole contract between it and the credential the server issues.

  /** Capture the create-key POST bodies and answer with a plausible created key. */
  async function captureCreates(
    page: Page,
    created: Parameters<typeof buildApiKey>[0] = {}
  ): Promise<unknown[]> {
    const bodies: unknown[] = [];
    await page.route(/\/api\/runtime(?:\/[^/]+)?\/api-keys$/, async (route) => {
      if (route.request().method() !== 'POST') return route.fallback();
      bodies.push(route.request().postDataJSON());
      await route.fulfill({
        status: 201,
        contentType: 'application/json',
        body: JSON.stringify({
          ...buildApiKey(created),
          key: 'rt_org_mocked_e2e_deadbeef',
        }),
      });
    });
    return bodies;
  }

  /** Days between now and an ISO timestamp, rounded — the presets are day-granular. */
  const daysUntil = (iso: string) =>
    Math.round((new Date(iso).getTime() - Date.now()) / 86_400_000);

  test('defaults to a 90-day key with no scope', async ({ page, mockApi }) => {
    await mockApi.bootstrap(page);
    await mockApi.apiKeys.list(page, []);
    const bodies = await captureCreates(page, { name: 'CI key' });

    const view = new SettingsPage(page);
    await view.goto();
    await page.getByRole('button', { name: /new api key/i }).click();
    await page.getByLabel('Name').fill('CI key');
    await page.getByRole('button', { name: /^create$/i }).click();

    await expect.poll(() => bodies.length).toBe(1);
    const body = bodies[0] as {
      name: string;
      expires_at: string;
      scope?: string;
    };
    expect(body.name).toBe('CI key');
    // Untouched, the dialog produces a bounded key — the default is deliberately not
    // "no expiration", which is what every key created before this control carried.
    expect(daysUntil(body.expires_at)).toBe(90);
    expect(body.scope).toBeUndefined();
  });

  test('creates a read-only key when the checkbox is ticked', async ({
    page,
    mockApi,
  }) => {
    await mockApi.bootstrap(page);
    await mockApi.apiKeys.list(page, []);
    const bodies = await captureCreates(page, {
      name: 'MCP reader',
      scope: 'read_only',
    });

    const view = new SettingsPage(page);
    await view.goto();
    await page.getByRole('button', { name: /new api key/i }).click();
    await page.getByLabel('Name').fill('MCP reader');
    await page.getByLabel('Read-only').check();
    await page.getByRole('button', { name: /^create$/i }).click();

    await expect.poll(() => bodies.length).toBe(1);
    const body = bodies[0] as { name: string; scope: string };
    expect(body.name).toBe('MCP reader');
    expect(body.scope).toBe('read_only');
  });

  test('omits expires_at entirely when No expiration is chosen', async ({
    page,
    mockApi,
  }) => {
    await mockApi.bootstrap(page);
    await mockApi.apiKeys.list(page, []);
    const bodies = await captureCreates(page, { name: 'Permanent key' });

    const view = new SettingsPage(page);
    await view.goto();
    await page.getByRole('button', { name: /new api key/i }).click();
    await page.getByLabel('Name').fill('Permanent key');
    await page.getByLabel('Expiration').click();
    await page.getByRole('option', { name: 'No expiration' }).click();
    await page.getByRole('button', { name: /^create$/i }).click();

    await expect.poll(() => bodies.length).toBe(1);
    // Absent, not null: a permanent key is stored exactly like a pre-expiry one.
    expect(bodies[0]).toEqual({ name: 'Permanent key' });
  });

  test('sends the chosen preset', async ({ page, mockApi }) => {
    await mockApi.bootstrap(page);
    await mockApi.apiKeys.list(page, []);
    const bodies = await captureCreates(page, { name: 'Short key' });

    const view = new SettingsPage(page);
    await view.goto();
    await page.getByRole('button', { name: /new api key/i }).click();
    await page.getByLabel('Name').fill('Short key');
    await page.getByLabel('Expiration').click();
    await page.getByRole('option', { name: '30 days' }).click();
    await page.getByRole('button', { name: /^create$/i }).click();

    await expect.poll(() => bodies.length).toBe(1);
    expect(daysUntil((bodies[0] as { expires_at: string }).expires_at)).toBe(
      30
    );
  });
});
