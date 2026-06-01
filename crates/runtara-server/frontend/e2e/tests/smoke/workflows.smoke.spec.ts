import { test, expect } from '@playwright/test';
import { appPath, appPathPattern } from '../../utils/app-path';

/**
 * Workflows Smoke Tests
 * Tests core workflow management functionality with real API calls
 */

test.describe('Workflows Smoke Tests', () => {
  test('workflows page loads with content', async ({ page }) => {
    await page.goto(appPath('/workflows'));
    await page.waitForLoadState('networkidle');

    // Page header is a console toolbar breadcrumb
    await expect(
      page
        .getByRole('navigation', { name: 'Breadcrumb' })
        .getByText('Workflows', { exact: true })
    ).toBeVisible();
  });

  test('new workflow button is present', async ({ page }) => {
    await page.goto(appPath('/workflows'));
    await page.waitForLoadState('networkidle');

    // New workflow button should be present
    await expect(
      page.getByRole('link', { name: /new workflow/i })
    ).toBeVisible();
  });

  test('can navigate to create workflow page', async ({ page }) => {
    await page.goto(appPath('/workflows'));
    await page.waitForLoadState('networkidle');

    // Click create button
    await page.getByRole('link', { name: /new workflow/i }).click();

    // Should navigate to create page
    await expect(page).toHaveURL(appPathPattern('/workflows/create'));
  });

  test('page renders without crash', async ({ page }) => {
    await page.goto(appPath('/workflows'));
    await page.waitForLoadState('networkidle');

    // Page should be functional
    await expect(page.locator('body')).toBeVisible();
    await expect(page.locator('main')).toBeVisible();
  });
});
