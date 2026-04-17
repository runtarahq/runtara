import { test, expect } from '@playwright/test';

/**
 * Connection CRUD E2E Tests
 *
 * Full write path:
 * Browser form -> Frontend -> Gateway(:8080) -> Runtime API(:7001) -> PostgreSQL -> read back
 *
 * Requires: full local stack running
 */

const TEST_CONNECTION_TITLE = `E2E Test SFTP ${Date.now()}`;

/**
 * Helper: delete a connection card by its title.
 * EntityTile renders actions twice (desktop + mobile), so we use
 * the visible delete button to avoid strict-mode violations.
 */
async function deleteConnectionByTitle(
  page: import('@playwright/test').Page,
  title: string
) {
  const card = page.locator('article').filter({ hasText: title });
  await card.getByTitle('Delete connection').first().click();
  // Confirm in the dialog
  await page.getByRole('button', { name: /delete connection/i }).click();
  await expect(card).not.toBeVisible({ timeout: 10000 });
}

test.describe.serial('Connection Create + Delete Flow', () => {
  test('create an SFTP connection through the full stack', async ({ page }) => {
    // 1. Navigate to connections page
    await page.goto('/connections');
    await page.waitForLoadState('networkidle');

    // 2. Open the "New connection" modal
    await page.getByRole('button', { name: /new connection/i }).click();
    await expect(page.getByRole('dialog')).toBeVisible();

    // 3. Select SFTP integration type
    const dialog = page.getByRole('dialog');
    await dialog.getByText('SFTP').click();

    // 4. Should navigate to the create form
    await expect(page).toHaveURL(/\/connections\/sftp\/create/);
    await page.waitForLoadState('networkidle');

    // 5. Fill in the connection form
    // Title field
    await page.getByLabel('Title').fill(TEST_CONNECTION_TITLE);

    // Server details — Host is required
    await page.getByLabel('Host').fill('sftp.example.com');

    // Port may have a default value of 22, fill anyway
    const portField = page.getByLabel('Port');
    if (await portField.isVisible()) {
      await portField.clear();
      await portField.fill('22');
    }

    // Authentication — Username
    await page.getByLabel('Username').fill('testuser');

    // Password field is in "Key-based Authentication" section and its input
    // doesn't have proper label association (PasswordField wraps input in a div),
    // so getByLabel won't work. Navigate from the text label instead.
    await page.getByText('Key-based Authentication').scrollIntoViewIfNeeded();
    await page
      .getByText('Password', { exact: true })
      .locator('..')
      .locator('input')
      .fill('testpass123');

    // 6. Submit the form
    await page.getByRole('button', { name: 'Create connection' }).click();

    // 7. Should redirect back to connections list
    await expect(page).toHaveURL('/connections', { timeout: 10000 });

    // 8. Success toast should appear
    await expect(page.getByText(/connection has been created/i)).toBeVisible({
      timeout: 5000,
    });

    // 9. The new connection should appear in the list
    await page.waitForLoadState('networkidle');
    await expect(page.getByText(TEST_CONNECTION_TITLE)).toBeVisible();

    // 10. Verify integration type badge shows on the new card
    await expect(
      page.getByRole('heading', { name: TEST_CONNECTION_TITLE })
    ).toBeVisible();
  });

  test('delete the created connection through the full stack', async ({
    page,
  }) => {
    // 1. Navigate to connections page
    await page.goto('/connections');
    await page.waitForLoadState('networkidle');

    // 2. Verify our test connection exists
    await expect(page.getByText(TEST_CONNECTION_TITLE)).toBeVisible();

    // 3. Delete the connection (uses .first() because EntityTile renders
    //    actions twice — desktop hover + mobile always-visible)
    await deleteConnectionByTitle(page, TEST_CONNECTION_TITLE);

    // 4. Connection should be gone from the list
    await expect(page.getByText(TEST_CONNECTION_TITLE)).not.toBeVisible({
      timeout: 10000,
    });
  });
});
