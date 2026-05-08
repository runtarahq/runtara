import { test, expect } from '@playwright/test';
import { appPath } from '../../utils/app-path';

/**
 * Connections Smoke Tests
 * Tests core connection management functionality with real API calls
 */

test.describe('Connections Smoke Tests', () => {
  test('connections page loads with content', async ({ page }) => {
    await page.goto(appPath('/connections'));
    await page.waitForLoadState('networkidle');

    // Page header should be visible (kicker is in <p>, title in <h1>)
    await expect(
      page.getByRole('main').getByText('Connections', { exact: true })
    ).toBeVisible();
    await expect(
      page.getByRole('heading', { name: /manage connections/i })
    ).toBeVisible();
  });

  test('new connection button is present', async ({ page }) => {
    await page.goto(appPath('/connections'));
    await page.waitForLoadState('networkidle');

    // New connection button should be present (may show "Loading..." initially)
    const newConnectionBtn = page.getByRole('button', {
      name: /new connection|loading/i,
    });
    await expect(newConnectionBtn).toBeVisible();
  });

  test('page renders without crash', async ({ page }) => {
    await page.goto(appPath('/connections'));
    await page.waitForLoadState('networkidle');

    // Page should be functional
    await expect(page.locator('body')).toBeVisible();
    await expect(page.locator('main')).toBeVisible();
  });
});
