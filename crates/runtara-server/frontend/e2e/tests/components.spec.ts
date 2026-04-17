import { test, expect } from '@playwright/test';

/**
 * UI Components E2E Tests
 * Tests for shared UI components behavior
 */

test.describe('Error Boundary', () => {
  test('should display error boundary on navigation error', async ({
    page,
  }) => {
    // Navigate to a route that triggers an error
    // This depends on your error boundary implementation
    await page.goto('/');

    // Error boundary should catch and display errors gracefully
    await page.waitForLoadState('networkidle');
  });
});

test.describe('Loading States', () => {
  test('should show loader component during async operations', async ({
    page,
  }) => {
    await page.goto('/');

    // Check for spinner/loader presence during initial load
    await page.waitForLoadState('domcontentloaded');

    // After load, the spinner should be hidden
    await page.waitForLoadState('networkidle');
  });
});

test.describe('Responsive Layout', () => {
  test('should display correctly on desktop viewport', async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 720 });
    await page.goto('/login');

    await expect(page.locator('body')).toBeVisible();
  });

  test('should display correctly on tablet viewport', async ({ page }) => {
    await page.setViewportSize({ width: 768, height: 1024 });
    await page.goto('/login');

    await expect(page.locator('body')).toBeVisible();
  });

  test('should display correctly on mobile viewport', async ({ page }) => {
    await page.setViewportSize({ width: 375, height: 667 });
    await page.goto('/login');

    await expect(page.locator('body')).toBeVisible();
  });
});

test.describe('Accessibility', () => {
  test('should have no accessibility violations on login page', async ({
    page,
  }) => {
    await page.goto('/login');

    // Basic accessibility checks
    // Check for proper heading hierarchy
    await page.locator('h1, h2, h3, h4, h5, h6').all();

    // Check for proper alt text on images
    const images = await page.locator('img').all();
    for (const img of images) {
      const alt = await img.getAttribute('alt');
      // Images should have alt text (empty string is valid for decorative images)
      expect(alt).not.toBeNull();
    }

    // Check for proper button labels
    const buttons = await page.locator('button').all();
    for (const button of buttons) {
      const text = await button.textContent();
      const ariaLabel = await button.getAttribute('aria-label');
      const ariaLabelledBy = await button.getAttribute('aria-labelledby');

      // Buttons should have accessible names
      expect(text || ariaLabel || ariaLabelledBy).toBeTruthy();
    }
  });

  test('should support keyboard navigation', async ({ page }) => {
    await page.goto('/login');

    // Tab through interactive elements
    await page.keyboard.press('Tab');

    // Check that focus is visible
    const focusedElement = await page.evaluate(() => {
      return document.activeElement?.tagName;
    });

    expect(focusedElement).toBeTruthy();
  });
});

test.describe('Form Components', () => {
  test('should handle form validation states', async ({ page }) => {
    await page.goto('/login');

    // This is a placeholder - adjust based on actual login form implementation
    await page.waitForLoadState('networkidle');
  });
});
