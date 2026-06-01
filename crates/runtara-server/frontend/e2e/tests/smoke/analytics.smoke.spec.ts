import { test, expect } from '@playwright/test';
import { appPath } from '../../utils/app-path';

/**
 * Analytics Smoke Tests
 * Tests core analytics dashboard functionality with real API calls
 */

test.describe('Analytics Smoke Tests', () => {
  test('analytics dashboard loads with content', async ({ page }) => {
    await page.goto(appPath('/analytics'));
    await page.waitForLoadState('networkidle');

    // Page header is a console toolbar breadcrumb (Analytics / Usage)
    const breadcrumb = page.getByRole('navigation', { name: 'Breadcrumb' });
    await expect(breadcrumb.getByText('Analytics', { exact: true })).toBeVisible();
    await expect(breadcrumb.getByText('Usage', { exact: true })).toBeVisible();
  });

  test('refresh button is present', async ({ page }) => {
    await page.goto(appPath('/analytics'));
    await page.waitForLoadState('networkidle');

    // Refresh button should be present
    await expect(page.getByRole('button', { name: /refresh/i })).toBeVisible();
  });

  test('page renders without crash', async ({ page }) => {
    await page.goto(appPath('/analytics'));
    await page.waitForLoadState('networkidle');

    // Page should be functional
    await expect(page.locator('body')).toBeVisible();
    await expect(page.locator('main')).toBeVisible();
  });
});
