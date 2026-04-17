import { Page, Locator, expect } from '@playwright/test';

/**
 * Common test helper utilities for e2e tests
 */

/**
 * Wait for page to be fully loaded (no network activity)
 */
export async function waitForPageLoad(page: Page): Promise<void> {
  await page.waitForLoadState('networkidle');
}

/**
 * Wait for a specific element to be visible and stable
 */
export async function waitForElement(
  page: Page,
  selector: string,
  options?: { timeout?: number }
): Promise<Locator> {
  const element = page.locator(selector);
  await element.waitFor({
    state: 'visible',
    timeout: options?.timeout ?? 5000,
  });
  return element;
}

/**
 * Fill a form field with a value, clearing existing content first
 */
export async function fillField(
  page: Page,
  selector: string,
  value: string
): Promise<void> {
  const field = page.locator(selector);
  await field.clear();
  await field.fill(value);
}

/**
 * Click a button and wait for navigation or response
 */
export async function clickAndWait(
  page: Page,
  selector: string,
  waitFor: 'navigation' | 'networkidle' = 'networkidle'
): Promise<void> {
  if (waitFor === 'navigation') {
    await Promise.all([page.waitForNavigation(), page.click(selector)]);
  } else {
    await page.click(selector);
    await page.waitForLoadState('networkidle');
  }
}

/**
 * Select an option from a dropdown (Radix Select component)
 */
export async function selectOption(
  page: Page,
  triggerSelector: string,
  optionText: string
): Promise<void> {
  // Click the trigger to open the dropdown
  await page.click(triggerSelector);

  // Wait for the dropdown content to appear
  await page.waitForSelector('[role="listbox"]', { state: 'visible' });

  // Click the option with matching text
  await page.click(`[role="option"]:has-text("${optionText}")`);
}

/**
 * Check if an element contains specific text
 */
export async function expectTextContent(
  page: Page,
  selector: string,
  expectedText: string
): Promise<void> {
  const element = page.locator(selector);
  await expect(element).toContainText(expectedText);
}

/**
 * Check if an element is visible
 */
export async function expectVisible(
  page: Page,
  selector: string
): Promise<void> {
  const element = page.locator(selector);
  await expect(element).toBeVisible();
}

/**
 * Check if an element is not visible
 */
export async function expectNotVisible(
  page: Page,
  selector: string
): Promise<void> {
  const element = page.locator(selector);
  await expect(element).not.toBeVisible();
}

/**
 * Get all items in a list/table
 */
export async function getListItems(
  page: Page,
  listSelector: string,
  itemSelector: string
): Promise<Locator[]> {
  const list = page.locator(listSelector);
  await list.waitFor({ state: 'visible' });
  return list.locator(itemSelector).all();
}

/**
 * Wait for toast notification
 */
export async function waitForToast(
  page: Page,
  expectedText?: string
): Promise<void> {
  const toast = page.locator('[role="status"], [data-sonner-toast]');
  await toast.waitFor({ state: 'visible' });

  if (expectedText) {
    await expect(toast).toContainText(expectedText);
  }
}

/**
 * Close modal/dialog if open
 */
export async function closeModal(page: Page): Promise<void> {
  const closeButton = page.locator(
    '[role="dialog"] button[aria-label="Close"], [role="dialog"] [data-testid="close-button"]'
  );

  if (await closeButton.isVisible()) {
    await closeButton.click();
    await page.waitForSelector('[role="dialog"]', { state: 'hidden' });
  }
}

/**
 * Confirm a dialog/modal action
 */
export async function confirmDialog(page: Page): Promise<void> {
  const confirmButton = page.locator(
    '[role="alertdialog"] button:has-text("Confirm"), [role="alertdialog"] button:has-text("Yes"), [role="alertdialog"] button:has-text("Delete")'
  );
  await confirmButton.click();
  await page.waitForSelector('[role="alertdialog"]', { state: 'hidden' });
}

/**
 * Cancel a dialog/modal action
 */
export async function cancelDialog(page: Page): Promise<void> {
  const cancelButton = page.locator(
    '[role="alertdialog"] button:has-text("Cancel"), [role="alertdialog"] button:has-text("No")'
  );
  await cancelButton.click();
  await page.waitForSelector('[role="alertdialog"]', { state: 'hidden' });
}

/**
 * Take a screenshot with a descriptive name
 */
export async function takeScreenshot(
  page: Page,
  name: string,
  options?: { fullPage?: boolean }
): Promise<void> {
  await page.screenshot({
    path: `e2e-results/screenshots/${name}.png`,
    fullPage: options?.fullPage ?? false,
  });
}

/**
 * Get current URL path (without base URL)
 */
export function getCurrentPath(page: Page): string {
  const url = new URL(page.url());
  return url.pathname;
}

/**
 * Check if user is on a specific path
 */
export async function expectPath(
  page: Page,
  expectedPath: string
): Promise<void> {
  await expect(page).toHaveURL(new RegExp(`${expectedPath}$`));
}

/**
 * Type with delay (useful for autocomplete fields)
 */
export async function typeWithDelay(
  page: Page,
  selector: string,
  text: string,
  delay = 100
): Promise<void> {
  await page.locator(selector).pressSequentially(text, { delay });
}

/**
 * Scroll element into view
 */
export async function scrollIntoView(
  page: Page,
  selector: string
): Promise<void> {
  await page.locator(selector).scrollIntoViewIfNeeded();
}

/**
 * Wait for specific URL pattern
 */
export async function waitForUrl(
  page: Page,
  urlPattern: string | RegExp
): Promise<void> {
  await page.waitForURL(urlPattern);
}

/**
 * Get table row by text content
 */
export async function getTableRowByText(
  page: Page,
  text: string
): Promise<Locator> {
  return page.locator(`tr:has-text("${text}")`);
}

/**
 * Check table has specific number of rows
 */
export async function expectTableRowCount(
  page: Page,
  tableSelector: string,
  expectedCount: number
): Promise<void> {
  const rows = page.locator(`${tableSelector} tbody tr`);
  await expect(rows).toHaveCount(expectedCount);
}
