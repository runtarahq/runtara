import { expect, test } from '../../../fixtures';
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
      workflowBreakdown: [],
    });
    await mockApi.analytics.system(page, {
      success: true,
      message: 'ok',
      data: {
        cpu: { physicalCores: 8, logicalCores: 16, architecture: 'x86_64' },
        memory: {
          availableBytes: 4_000_000_000,
          availableForWorkflowsBytes: 3_200_000_000,
          totalBytes: 8_000_000_000,
        },
        disk: {
          availableBytes: 50_000_000_000,
          totalBytes: 100_000_000_000,
          path: '/data',
        },
      },
    });

    const view = new AnalyticsUsagePage(page);
    await view.goto();

    await view.expectHeading(/usage/i);
    await expect(page.getByText('16 cores')).toBeVisible();
    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('analytics-usage');
  });
});
