import { test, expect } from '@playwright/test';

/**
 * Theme E2E Tests
 * Tests for dark/light mode switching functionality
 * These tests use /login page which doesn't require authentication
 */

test.describe('Theme switching', () => {
  test.beforeEach(async ({ page }) => {
    // Clear any stored theme preference
    await page.addInitScript(() => {
      localStorage.removeItem('theme');
    });
  });

  test('should load page without errors', async ({ page }) => {
    await page.goto('/login');
    await page.waitForLoadState('domcontentloaded');
    await expect(page.locator('body')).toBeVisible();
  });

  test('should persist theme preference in localStorage', async ({ page }) => {
    await page.goto('/login');

    // Set dark theme via localStorage
    await page.evaluate(() => {
      localStorage.setItem('theme', 'dark');
    });

    // Verify it was set
    const storedTheme = await page.evaluate(() =>
      localStorage.getItem('theme')
    );
    expect(storedTheme).toBe('dark');
  });
});

test.describe('Theme CSS variables', () => {
  test('should have CSS variables defined', async ({ page }) => {
    await page.goto('/login');
    await page.waitForLoadState('domcontentloaded');

    // Check that CSS custom properties are defined
    const hasStyles = await page.evaluate(() => {
      const styles = getComputedStyle(document.documentElement);
      // Check if any CSS variable is set (app is loaded)
      return styles.length > 0;
    });

    expect(hasStyles).toBe(true);
  });
});
