import { test } from '../../../fixtures';
import { AnalyticsRateLimitsPage } from '../../../pages/AnalyticsPages';

test.describe('Analytics / Rate limits (mocked)', () => {
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

    const view = new AnalyticsRateLimitsPage(page);
    await view.goto();

    await view.expectHeading(/rate limits/i);
    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('analytics-rate-limits');
  });
});
