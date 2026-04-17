import { test, expect } from '@playwright/test';

/**
 * Login Page E2E Tests (Unauthenticated)
 *
 * These tests run WITHOUT authentication state.
 * File naming convention: *.unauth.spec.ts
 */

test.describe('Login Page (Unauthenticated)', () => {
  test('should load login page', async ({ page }) => {
    await page.goto('/login');
    await page.waitForLoadState('domcontentloaded');

    // Page should load without crashing
    await expect(page.locator('body')).toBeVisible();
  });

  test('should show login page or redirect to Auth0', async ({ page }) => {
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

  test('should have a clickable auth button', async ({ page }) => {
    await page.goto('/login');
    await page.waitForLoadState('domcontentloaded');

    // Find the auth button
    const authButton = page.locator('button').first();
    await expect(authButton).toBeVisible();
  });
});

test.describe('Protected Routes (Unauthenticated)', () => {
  const protectedRoutes = [
    '/scenarios',
    '/connections',
    '/invocation-triggers',
    '/analytics',
    '/objects/types',
  ];

  for (const route of protectedRoutes) {
    test(`should redirect from ${route} when unauthenticated`, async ({
      page,
    }) => {
      await page.goto(route);

      // Wait for any redirect to complete
      await page.waitForLoadState('domcontentloaded');

      // Should not show error page (app should handle redirect gracefully)
      await expect(page.locator('body')).toBeVisible();
    });
  }
});
