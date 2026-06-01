import { test, expect } from '@playwright/test';
import { appPath, appPathPattern } from '../../utils/app-path';

/**
 * Triggers Smoke Tests
 * Tests core invocation trigger management functionality with real API calls
 */

test.describe('Triggers Smoke Tests', () => {
  test('triggers page loads with content', async ({ page }) => {
    await page.goto(appPath('/invocation-triggers'));
    await page.waitForLoadState('networkidle');

    // Page header is a console toolbar breadcrumb
    await expect(
      page
        .getByRole('navigation', { name: 'Breadcrumb' })
        .getByText('Triggers', { exact: true })
    ).toBeVisible();
  });

  test('new trigger button is present', async ({ page }) => {
    await page.goto(appPath('/invocation-triggers'));
    await page.waitForLoadState('networkidle');

    // New trigger button should be present
    await expect(
      page.getByRole('link', { name: /new trigger/i })
    ).toBeVisible();
  });

  test('can navigate to create trigger page', async ({ page }) => {
    await page.goto(appPath('/invocation-triggers'));
    await page.waitForLoadState('networkidle');

    // Click create button
    await page.getByRole('link', { name: /new trigger/i }).click();

    // Should navigate to create page
    await expect(page).toHaveURL(appPathPattern('/invocation-triggers/create'));
  });

  test('page renders without crash', async ({ page }) => {
    await page.goto(appPath('/invocation-triggers'));
    await page.waitForLoadState('networkidle');

    // Page should be functional
    await expect(page.locator('body')).toBeVisible();
    await expect(page.locator('main')).toBeVisible();
  });
});
