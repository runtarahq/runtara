import { test } from '../../../fixtures';
import { AnalyticsSystemPage } from '../../../pages/AnalyticsPages';

test.describe('Analytics / System (mocked)', () => {
  test('renders dashboard, a11y + snapshot', async ({
    page,
    mockApi,
    runA11y,
  }) => {
    await mockApi.bootstrap(page);
    await mockApi.analytics.system(page, {
      cpu: {
        usagePercent: 24,
        physicalCores: 8,
        logicalCores: 16,
        architecture: 'x86_64',
      },
      memory: { usedBytes: 1_000_000_000, totalBytes: 8_000_000_000 },
      uptimeSeconds: 3600,
      version: 'test',
    });

    const view = new AnalyticsSystemPage(page);
    await view.goto();

    await view.expectHeading(/^system$/i);
    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('analytics-system');
  });
});
