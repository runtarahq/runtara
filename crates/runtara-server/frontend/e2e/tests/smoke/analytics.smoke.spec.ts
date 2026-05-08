import { test, expect } from '@playwright/test';

/**
 * Analytics Smoke Tests
 * Tests core analytics dashboard functionality with real API calls
 */

test.describe('Analytics Smoke Tests', () => {
  test('analytics dashboard loads with content', async ({ page }) => {
    await page.goto('/analytics');
    await page.waitForLoadState('networkidle');

    // Page header should be visible (kicker "Analytics" is in <p>, title "Usage" in <h1>)
    await expect(
      page.getByRole('main').getByText('Analytics', { exact: true })
    ).toBeVisible();
    await expect(page.getByRole('heading', { name: /usage/i })).toBeVisible();
  });

  test('refresh button is present', async ({ page }) => {
    await page.goto('/analytics');
    await page.waitForLoadState('networkidle');

    // Refresh button should be present
    await expect(page.getByRole('button', { name: /refresh/i })).toBeVisible();
  });

  test('page renders without crash', async ({ page }) => {
    await page.goto('/analytics');
    await page.waitForLoadState('networkidle');

    // Page should be functional
    await expect(page.locator('body')).toBeVisible();
    await expect(page.locator('main')).toBeVisible();
  });
});
