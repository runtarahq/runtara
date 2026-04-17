import { test, expect } from '@playwright/test';

/**
 * Scenarios Smoke Tests
 * Tests core scenario management functionality with real API calls
 */

test.describe('Scenarios Smoke Tests', () => {
  test('scenarios page loads with content', async ({ page }) => {
    await page.goto('/scenarios');
    await page.waitForLoadState('networkidle');

    // Page header should be visible (kicker is in <p>, title in <h1>)
    await expect(
      page.getByRole('paragraph').filter({ hasText: 'Scenarios' })
    ).toBeVisible();
    await expect(
      page.getByRole('heading', { name: /build and iterate automation flows/i })
    ).toBeVisible();
  });

  test('new scenario button is present', async ({ page }) => {
    await page.goto('/scenarios');
    await page.waitForLoadState('networkidle');

    // New scenario button should be present
    await expect(
      page.getByRole('link', { name: /new scenario/i })
    ).toBeVisible();
  });

  test('can navigate to create scenario page', async ({ page }) => {
    await page.goto('/scenarios');
    await page.waitForLoadState('networkidle');

    // Click create button
    await page.getByRole('link', { name: /new scenario/i }).click();

    // Should navigate to create page
    await expect(page).toHaveURL(/\/scenarios\/create/);
  });

  test('page renders without crash', async ({ page }) => {
    await page.goto('/scenarios');
    await page.waitForLoadState('networkidle');

    // Page should be functional
    await expect(page.locator('body')).toBeVisible();
    await expect(page.locator('main')).toBeVisible();
  });
});
