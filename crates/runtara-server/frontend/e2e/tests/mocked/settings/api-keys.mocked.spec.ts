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
});
