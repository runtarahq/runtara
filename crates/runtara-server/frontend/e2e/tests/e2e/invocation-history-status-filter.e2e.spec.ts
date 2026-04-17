import { test, expect } from '@playwright/test';

/**
 * Invocation History Status Filter E2E Test — SYN-128
 *
 * Verifies that the Status filter dropdown on the Invocation History page
 * shows only valid statuses that can actually be returned by the backend.
 *
 * The backend maps Runtara InstanceStatus to ExecutionStatus. Only these
 * statuses are ever produced: Queued, Running, Completed, Failed, Cancelled.
 * "Compiling" and "Timeout" exist in the enum but are never returned.
 *
 * Requires: full local stack running (gateway, runtime, management, frontend)
 */

const VALID_STATUSES = [
  'All statuses',
  'Queued',
  'Running',
  'Completed',
  'Failed',
  'Cancelled',
];

const INVALID_STATUSES = ['Compiling', 'Timeout'];

test.describe('Invocation History Status Filter (SYN-128)', () => {
  test('status filter dropdown shows only valid statuses', async ({ page }) => {
    await page.goto('/invocation-history');
    await page.waitForLoadState('networkidle');

    // Verify we're on the Invocation History page
    await expect(page.getByText('Invocation History').first()).toBeVisible({
      timeout: 15000,
    });

    // Open the Status dropdown (second combobox, shows "All statuses")
    const statusTrigger = page
      .getByRole('combobox')
      .filter({ hasText: 'All statuses' });
    await expect(statusTrigger).toBeVisible();
    await statusTrigger.click();

    // Wait for dropdown content to appear
    const listbox = page.getByRole('listbox');
    await expect(listbox).toBeVisible({ timeout: 5000 });

    // Get all option texts
    const options = listbox.getByRole('option');
    const optionTexts = await options.allTextContents();

    // Verify all valid statuses are present
    for (const status of VALID_STATUSES) {
      expect(
        optionTexts,
        `Expected status "${status}" to be in the dropdown`
      ).toContain(status);
    }

    // Verify invalid statuses are NOT present
    for (const status of INVALID_STATUSES) {
      expect(
        optionTexts,
        `Status "${status}" should NOT be in the dropdown`
      ).not.toContain(status);
    }

    // Verify the exact count (All statuses + 5 valid statuses = 6)
    expect(optionTexts).toHaveLength(6);
  });
});
