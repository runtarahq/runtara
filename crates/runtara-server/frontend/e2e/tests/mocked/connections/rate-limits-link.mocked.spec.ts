import { expect } from '@playwright/test';
import { test, buildConnection } from '../../../fixtures';
import { appPath } from '../../../utils/app-path';

/**
 * Rate limits live under Connections: the table's usage cell links straight to
 * that connection's history, and the toolbar links to the whole dashboard.
 */
test.describe('Connections / Rate limits links (mocked)', () => {
  test('a row usage link opens that connection on the rate limits page', async ({
    page,
    mockApi,
  }) => {
    const conn = buildConnection({
      id: 'conn_limited',
      integrationId: 'http',
      title: 'Limited API',
      rateLimitStats: {
        interval: '24h',
        totalRequests: 1200,
        rateLimitedCount: 3,
        rateLimitedPercent: 0.25,
        retryCount: 3,
      },
    });

    await mockApi.bootstrap(page);
    await mockApi.connections.list(page, [conn]);
    await mockApi.connections.types(page, []);
    await page.route(
      /\/api\/runtime(?:\/[^/]+)?\/rate-limits(?:\?[^/]*)?$/,
      (route) =>
        route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            success: true,
            data: [
              {
                connectionId: conn.id,
                connectionTitle: conn.title,
                integrationId: 'http',
                config: null,
                state: { available: true },
                metrics: { isRateLimited: false },
                periodStats: null,
              },
            ],
          }),
        })
    );
    await page.route(
      /\/connections\/conn_limited\/rate-limit-history/,
      (route) =>
        route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({ success: true, data: [] }),
        })
    );
    await page.route(
      /\/connections\/conn_limited\/rate-limit-timeline/,
      (route) =>
        route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({ success: true, data: { buckets: [] } }),
        })
    );

    await page.goto(appPath('/connections'));
    await expect(
      page.getByRole('link', { name: /^rate limits$/i })
    ).toHaveAttribute('href', /\/connections\/rate-limits$/);

    await page
      .getByRole('link', { name: /rate limits for limited api/i })
      .click();

    await expect(page).toHaveURL(
      /\/connections\/rate-limits\?.*connection=conn_limited/
    );
    await expect(
      page.getByText('Rate Limit History: Limited API')
    ).toBeVisible();
  });

  test('the old analytics address redirects', async ({ page, mockApi }) => {
    await mockApi.bootstrap(page);
    await mockApi.connections.list(page, []);
    await page.route(
      /\/api\/runtime(?:\/[^/]+)?\/rate-limits(?:\?[^/]*)?$/,
      (route) =>
        route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({ success: true, data: [] }),
        })
    );

    await page.goto(appPath('/analytics/rate-limits'));
    await expect(page).toHaveURL(/\/connections\/rate-limits/);
  });
});
