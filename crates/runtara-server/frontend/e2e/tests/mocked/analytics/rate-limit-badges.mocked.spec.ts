import { expect } from '@playwright/test';
import { test } from '../../../fixtures';
import { AnalyticsRateLimitsPage } from '../../../pages/AnalyticsPages';

/**
 * SYN-495 regression guard: the rate-limit dashboard badge must reflect the
 * TRUE protection state. In particular a connection with no config must read
 * "No limit" (neutral), never the green "OK" success badge that implies
 * protection that isn't there.
 */

const CONFIG = {
  requestsPerSecond: 20,
  burstSize: 40,
  retryOnLimit: true,
  maxRetries: 3,
  maxWaitMs: 60000,
};

type Status = Record<string, unknown>;

function status(over: Status): Status {
  return {
    connectionId: 'c',
    connectionTitle: 'Connection',
    integrationId: 'stripe_api_key',
    config: null,
    state: { available: true },
    metrics: { isRateLimited: false },
    periodStats: null,
    ...over,
  };
}

test.describe('Analytics / Rate limits badges (mocked)', () => {
  test('each card shows a badge that matches its real protection state', async ({
    page,
    mockApi,
  }) => {
    await mockApi.bootstrap(page);
    await mockApi.connections.list(page, []);

    const statuses = [
      status({
        connectionId: 'no-config',
        connectionTitle: 'Unprotected Stripe',
        config: null,
      }),
      status({
        connectionId: 'redis-down',
        connectionTitle: 'Configured Slack',
        integrationId: 'slack_bot',
        config: CONFIG,
        state: { available: false },
      }),
      status({
        connectionId: 'limited',
        connectionTitle: 'Busy HubSpot',
        integrationId: 'hubspot_access_token',
        config: CONFIG,
        state: { available: true },
        metrics: { isRateLimited: true },
      }),
      status({
        connectionId: 'healthy',
        connectionTitle: 'Healthy OpenAI',
        integrationId: 'openai_api_key',
        config: CONFIG,
        state: { available: true },
        metrics: { isRateLimited: false, capacityPercent: 80 },
      }),
    ];

    // The dashboard maps the LIST endpoint to one RateLimitCard per connection.
    await page.route(
      /\/api\/runtime(?:\/[^/]+)?\/rate-limits(?:\?[^/]*)?$/,
      (route) =>
        route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({ success: true, data: statuses }),
        })
    );

    const view = new AnalyticsRateLimitsPage(page);
    await view.goto();
    await view.expectHeading(/rate limits/i);

    // Every honest badge state is present...
    await expect(page.getByText('No limit')).toBeVisible();
    await expect(page.getByText('Not enforced')).toBeVisible();
    await expect(page.getByText('Rate limited')).toBeVisible();
    await expect(page.getByText('OK')).toBeVisible();

    // ...and the unprotected connection's card never claims to be "OK".
    const unprotectedCard = page
      .locator('div', { hasText: 'Unprotected Stripe' })
      .filter({ hasText: 'No limit' })
      .first();
    await expect(unprotectedCard).toBeVisible();
    await expect(unprotectedCard.getByText('OK')).toHaveCount(0);
  });
});
