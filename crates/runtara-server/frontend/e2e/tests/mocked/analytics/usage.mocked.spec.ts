import { test } from '../../../fixtures';
import { AnalyticsUsagePage } from '../../../pages/AnalyticsPages';

test.describe('Analytics / Usage (mocked)', () => {
  test('renders dashboard, a11y + snapshot', async ({
    page,
    mockApi,
    runA11y,
  }) => {
    await mockApi.bootstrap(page);
    await mockApi.analytics.tenantMetrics(page, {
      totalExecutions: 42,
      successfulExecutions: 40,
      failedExecutions: 2,
      executionTimeSeries: [],
      scenarioBreakdown: [],
    });

    const view = new AnalyticsUsagePage(page);
    await view.goto();

    await view.expectHeading(/usage/i);
    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('analytics-usage');
  });
});
