import { test, expect } from '@playwright/test';

/**
 * Authentication E2E Tests
 * Tests for login page and authentication state
 *
 * Note: These tests don't require actual Auth0 login since
 * they test the app's behavior, not Auth0 itself.
 */

test.describe('Authentication Flow', () => {
  test.describe('Login Page', () => {
    test('should load login page', async ({ page }) => {
      await page.goto('/login');
      await page.waitForLoadState('domcontentloaded');

      await expect(page.locator('body')).toBeVisible();
    });

    test('should have proper page title', async ({ page }) => {
      await page.goto('/login');
      await page.waitForLoadState('domcontentloaded');

      // Check page has a title (non-empty)
      const title = await page.title();
      expect(title).toBeTruthy();
    });

    test('should display login content or redirect to Auth0', async ({
      page,
    }) => {
      await page.goto('/login');
      await page.waitForLoadState('domcontentloaded');

      // Either on app login page or redirected to the configured OIDC authority
      const url = page.url();
      const authorityHost = process.env.VITE_OIDC_AUTHORITY
        ? new URL(process.env.VITE_OIDC_AUTHORITY).host
        : '';
      const isOnAuth0 = authorityHost !== '' && url.includes(authorityHost);
      const isOnApp = url.includes('localhost:8081');

      expect(isOnAuth0 || isOnApp).toBe(true);
    });
  });

  test.describe('Session Storage', () => {
    test('should be able to set localStorage values', async ({ page }) => {
      await page.goto('/login');

      await page.evaluate(() => {
        localStorage.setItem('test_key', 'test_value');
      });

      const value = await page.evaluate(() => localStorage.getItem('test_key'));
      expect(value).toBe('test_value');
    });

    test('should clear storage correctly', async ({ page }) => {
      await page.goto('/login');

      await page.evaluate(() => {
        localStorage.setItem('test_key', 'test_value');
        localStorage.clear();
      });

      const value = await page.evaluate(() => localStorage.getItem('test_key'));
      expect(value).toBeNull();
    });
  });
});
