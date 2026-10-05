import { test } from '../../../fixtures';
import { ConnectionRateLimitsPage } from '../../../pages/ConnectionsPage';

test.describe('Connections / Rate limits (mocked)', () => {
  test('renders dashboard, a11y + snapshot', async ({
    page,
    mockApi,
    runA11y,
  }) => {
    await mockApi.bootstrap(page);
    await mockApi.connections.list(page, []);
    await mockApi.analytics.rateLimits(page, {
      current: 0,
      limit: 100,
      windowSeconds: 60,
    });

    const view = new ConnectionRateLimitsPage(page);
    await view.goto();

    await view.expectHeading(/rate limits/i);
    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('connections-rate-limits');
  });
});
