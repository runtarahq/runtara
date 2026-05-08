import { test, expect } from '@playwright/test';

/**
 * Objects (Database) Smoke Tests
 * Tests core object type and instance management with real API calls
 */

test.describe('Objects Smoke Tests', () => {
  test('object types page loads with content', async ({ page }) => {
    await page.goto('/objects/types');
    await page.waitForLoadState('networkidle');

    // Page header should be visible (kicker is in <p>, title in <h1>)
    await expect(
      page.getByRole('main').getByText('Database', { exact: true })
    ).toBeVisible();
    await expect(
      page.getByRole('heading', { name: /object types/i })
    ).toBeVisible();
  });

  test('create object type button is present', async ({ page }) => {
    await page.goto('/objects/types');
    await page.waitForLoadState('networkidle');

    // Create button should be present (it's a button with onClick, not a link)
    await expect(
      page.getByRole('button', { name: /create object type/i })
    ).toBeVisible();
  });

  test('page renders without crash', async ({ page }) => {
    await page.goto('/objects/types');
    await page.waitForLoadState('networkidle');

    // Page should be functional
    await expect(page.locator('body')).toBeVisible();
    await expect(page.locator('main')).toBeVisible();
  });
});
