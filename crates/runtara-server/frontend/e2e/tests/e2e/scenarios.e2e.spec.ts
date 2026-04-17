import { test, expect } from '@playwright/test';

/**
 * Scenarios E2E Tests
 *
 * Full request path:
 * Browser -> Frontend(:8081) -> Gateway(:8080) -> Runtime API(:7001) -> PostgreSQL
 *
 * Requires: full local stack running
 */

test.describe('Scenarios E2E', () => {
  test('scenarios page loads with header from runtime API', async ({
    page,
  }) => {
    await page.goto('/scenarios');
    await page.waitForLoadState('networkidle');

    await expect(
      page.getByRole('main').getByText('Scenarios', { exact: true })
    ).toBeVisible();
    await expect(
      page.getByRole('heading', { name: /build and iterate automation flows/i })
    ).toBeVisible();
  });

  test('scenarios page shows content after loading', async ({ page }) => {
    await page.goto('/scenarios');
    await page.waitForLoadState('networkidle');

    // Either empty state or scenario list should be visible
    const emptyState = page.getByText(/no scenarios yet/i);
    const scenarioList = page.locator('article').first();
    await expect(emptyState.or(scenarioList)).toBeVisible();
  });

  test('new scenario button is present and functional', async ({ page }) => {
    await page.goto('/scenarios');
    await page.waitForLoadState('networkidle');

    await expect(
      page.getByRole('link', { name: /new scenario/i })
    ).toBeVisible();
  });

  test('can navigate to create scenario page', async ({ page }) => {
    await page.goto('/scenarios');
    await page.waitForLoadState('networkidle');

    await page.getByRole('link', { name: /new scenario/i }).click();
    await expect(page).toHaveURL(/\/scenarios\/create/);
  });

  test('scenarios page renders without API errors', async ({ page }) => {
    await page.goto('/scenarios');
    await page.waitForLoadState('networkidle');

    // Wait for API calls to complete
    await page.waitForTimeout(2000);

    // No network error alerts
    const errorAlert = page.locator('[role="alert"]');
    const alertCount = await errorAlert.count();
    if (alertCount > 0) {
      const alertText = await errorAlert.first().textContent();
      expect(alertText).not.toContain('Network Error');
      expect(alertText).not.toContain('Failed to fetch');
    }
  });
});
