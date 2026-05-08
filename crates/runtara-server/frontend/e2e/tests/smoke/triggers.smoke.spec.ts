import { test, expect } from '@playwright/test';

/**
 * Triggers Smoke Tests
 * Tests core invocation trigger management functionality with real API calls
 */

test.describe('Triggers Smoke Tests', () => {
  test('triggers page loads with content', async ({ page }) => {
    await page.goto('/invocation-triggers');
    await page.waitForLoadState('networkidle');

    // Page header should be visible (kicker is in <p>, title in <h1>)
    await expect(
      page.getByRole('main').getByText('Invocation triggers', { exact: true })
    ).toBeVisible();
    await expect(
      page.getByRole('heading', { name: /manage event sources/i })
    ).toBeVisible();
  });

  test('new trigger button is present', async ({ page }) => {
    await page.goto('/invocation-triggers');
    await page.waitForLoadState('networkidle');

    // New trigger button should be present
    await expect(
      page.getByRole('link', { name: /new trigger/i })
    ).toBeVisible();
  });

  test('can navigate to create trigger page', async ({ page }) => {
    await page.goto('/invocation-triggers');
    await page.waitForLoadState('networkidle');

    // Click create button
    await page.getByRole('link', { name: /new trigger/i }).click();

    // Should navigate to create page
    await expect(page).toHaveURL(/\/invocation-triggers\/create/);
  });

  test('page renders without crash', async ({ page }) => {
    await page.goto('/invocation-triggers');
    await page.waitForLoadState('networkidle');

    // Page should be functional
    await expect(page.locator('body')).toBeVisible();
    await expect(page.locator('main')).toBeVisible();
  });
});
