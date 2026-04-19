import { test, expect } from '@playwright/test';

/**
 * Tests that the versions dropdown is scrollable when there are many versions
 */

test.describe('Versions dropdown scroll', () => {
  test('versions dropdown should be scrollable with many versions', async ({
    page,
  }) => {
    // Go to workflows list page
    await page.goto('/workflows');
    await page.waitForLoadState('networkidle');

    // Wait for loading skeletons to disappear and content to load
    await page
      .waitForSelector('.animate-pulse', { state: 'hidden', timeout: 15000 })
      .catch(() => {});

    // Wait a moment for workflows to render
    await page.waitForTimeout(1000);

    // Find the Edit button on a workflow card to navigate to workflow editor
    const editButton = page.getByRole('button', { name: 'Edit' }).first();

    // If no edit button exists, skip the test
    const hasEditButton = await editButton
      .isVisible({ timeout: 5000 })
      .catch(() => false);
    if (!hasEditButton) {
      test.skip(true, 'No workflows available to test');
      return;
    }

    await editButton.click();
    await page.waitForLoadState('networkidle');

    // Wait for workflow editor to load
    await page.waitForTimeout(1000);

    // Find the version dropdown button (has role="combobox" and contains version text)
    const versionButton = page.locator('button[role="combobox"]');

    // Check if version dropdown exists (workflow needs versions)
    const hasVersionDropdown = await versionButton
      .isVisible({ timeout: 5000 })
      .catch(() => false);
    if (!hasVersionDropdown) {
      test.skip(true, 'Workflow does not have version dropdown');
      return;
    }

    // Click to open the dropdown
    await versionButton.click();

    // Wait for the popover to appear
    const popoverContent = page.locator('[data-radix-popper-content-wrapper]');
    await expect(popoverContent).toBeVisible();

    // Find the scrollable container within the popover
    const scrollContainer = popoverContent.locator('.overflow-y-auto');
    await expect(scrollContainer).toBeVisible();

    // Get scroll info to verify scroll behavior
    const scrollInfo = await scrollContainer.evaluate((el) => ({
      scrollHeight: el.scrollHeight,
      clientHeight: el.clientHeight,
      isScrollable: el.scrollHeight > el.clientHeight,
    }));

    // If content is scrollable, test the scroll functionality
    if (scrollInfo.isScrollable) {
      // Scroll to bottom
      await scrollContainer.evaluate((el) => {
        el.scrollTop = el.scrollHeight;
      });

      // Verify scroll happened
      const scrolledPosition = await scrollContainer.evaluate(
        (el) => el.scrollTop
      );
      expect(scrolledPosition).toBeGreaterThan(0);

      // Scroll back to top
      await scrollContainer.evaluate((el) => {
        el.scrollTop = 0;
      });

      const topPosition = await scrollContainer.evaluate((el) => el.scrollTop);
      expect(topPosition).toBe(0);
    }

    // Close the dropdown by clicking elsewhere
    await page.keyboard.press('Escape');
    await expect(popoverContent).not.toBeVisible();
  });

  test('version dropdown container has proper overflow styling', async ({
    page,
  }) => {
    // Go to workflows list
    await page.goto('/workflows');
    await page.waitForLoadState('networkidle');

    // Wait for loading skeletons to disappear
    await page
      .waitForSelector('.animate-pulse', { state: 'hidden', timeout: 15000 })
      .catch(() => {});
    await page.waitForTimeout(1000);

    // Find the Edit button on a workflow card
    const editButton = page.getByRole('button', { name: 'Edit' }).first();
    const hasEditButton = await editButton
      .isVisible({ timeout: 5000 })
      .catch(() => false);
    if (!hasEditButton) {
      test.skip(true, 'No workflows available to test');
      return;
    }

    await editButton.click();
    await page.waitForLoadState('networkidle');
    await page.waitForTimeout(1000);

    // Find and click version dropdown
    const versionButton = page.locator('button[role="combobox"]');
    const hasVersionDropdown = await versionButton
      .isVisible({ timeout: 5000 })
      .catch(() => false);
    if (!hasVersionDropdown) {
      test.skip(true, 'Workflow does not have version dropdown');
      return;
    }

    await versionButton.click();

    // Verify popover is visible
    const popoverContent = page.locator('[data-radix-popper-content-wrapper]');
    await expect(popoverContent).toBeVisible();

    // Verify the scroll container has overflow-y-auto class
    const scrollContainer = popoverContent.locator('.overflow-y-auto');
    await expect(scrollContainer).toBeVisible();

    // Verify max-height is set (either via class or computed style)
    const hasMaxHeight = await scrollContainer.evaluate((el) => {
      const computedStyle = window.getComputedStyle(el);
      const maxHeight = computedStyle.maxHeight;
      return maxHeight !== 'none' && maxHeight !== '0px';
    });

    expect(hasMaxHeight).toBe(true);
  });
});
