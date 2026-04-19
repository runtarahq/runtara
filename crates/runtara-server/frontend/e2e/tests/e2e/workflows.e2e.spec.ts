import { test, expect } from '@playwright/test';

/**
 * Workflows E2E Tests
 *
 * Full request path:
 * Browser -> Frontend(:8081) -> Gateway(:8080) -> Runtime API(:7001) -> PostgreSQL
 *
 * Requires: full local stack running
 */

test.describe('Workflows E2E', () => {
  test('workflows page loads with header from runtime API', async ({
    page,
  }) => {
    await page.goto('/workflows');
    await page.waitForLoadState('networkidle');

    await expect(
      page.getByRole('main').getByText('Workflows', { exact: true })
    ).toBeVisible();
    await expect(
      page.getByRole('heading', { name: /build and iterate automation flows/i })
    ).toBeVisible();
  });

  test('workflows page shows content after loading', async ({ page }) => {
    await page.goto('/workflows');
    await page.waitForLoadState('networkidle');

    // Either empty state or workflow list should be visible
    const emptyState = page.getByText(/no workflows yet/i);
    const workflowList = page.locator('article').first();
    await expect(emptyState.or(workflowList)).toBeVisible();
  });

  test('new workflow button is present and functional', async ({ page }) => {
    await page.goto('/workflows');
    await page.waitForLoadState('networkidle');

    await expect(
      page.getByRole('link', { name: /new workflow/i })
    ).toBeVisible();
  });

  test('can navigate to create workflow page', async ({ page }) => {
    await page.goto('/workflows');
    await page.waitForLoadState('networkidle');

    await page.getByRole('link', { name: /new workflow/i }).click();
    await expect(page).toHaveURL(/\/workflows\/create/);
  });

  test('workflows page renders without API errors', async ({ page }) => {
    await page.goto('/workflows');
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
