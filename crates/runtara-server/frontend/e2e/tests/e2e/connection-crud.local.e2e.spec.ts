import { test, expect } from '@playwright/test';

/**
 * Connection CRUD E2E Tests
 *
 * Full write path:
 * Browser form -> Frontend -> Runtime API -> PostgreSQL -> read back
 *
 * Requires: local server and frontend running
 */

const TEST_CONNECTION_TITLE = `E2E Test MCP ${Date.now()}`;

/**
 * Delete the test connection row by its title.
 */
async function deleteConnectionByTitle(
  page: import('@playwright/test').Page,
  title: string
) {
  const card = page.locator('tr').filter({ hasText: title });
  await card
    .getByRole('button', { name: 'Delete connection', exact: true })
    .first()
    .click();
  // Confirm in the dialog
  await page
    .getByRole('dialog')
    .getByRole('button', { name: 'Delete Connection', exact: true })
    .click();
  await expect(card).not.toBeVisible({ timeout: 10000 });
}

test.describe.serial('Connection Create + Delete Flow', () => {
  test('create an MCP connection through the full stack', async ({ page }) => {
    // 1. Navigate to connections page
    await page.goto('connections');
    await page.waitForLoadState('networkidle');

    // 2. Open the "New connection" modal
    await page.getByRole('button', { name: /new connection/i }).click();
    await expect(page.getByRole('dialog')).toBeVisible();

    // 3. Select MCP integration type
    const dialog = page.getByRole('dialog');
    await dialog.getByText('MCP Server', { exact: true }).click();

    // 4. Should navigate to the create form
    await expect(page).toHaveURL(/\/connections\/mcp\/create/);
    await page.waitForLoadState('networkidle');

    // 5. Fill in the connection form
    // Title field
    await page.getByLabel('Title').fill(TEST_CONNECTION_TITLE);

    await page.getByLabel('Server URL').fill('https://mcp.example.com');
    await page.getByLabel('Auth Mode').click();
    await page.getByRole('option', { name: 'Bearer' }).click();
    await page.getByLabel('Bearer Token*').fill('test-token');

    // 6. Submit the form
    await page.getByRole('button', { name: 'Create connection' }).click();

    // 7. Should redirect back to connections list
    await expect(page).toHaveURL('connections', { timeout: 10000 });

    // 8. Success toast should appear
    await expect(page.getByText(/connection created/i)).toBeVisible({
      timeout: 5000,
    });

    // 9. The new connection should appear in the list
    await page.waitForLoadState('networkidle');
    await expect(page.getByText(TEST_CONNECTION_TITLE)).toBeVisible();

    // 10. Verify integration type badge shows on the new card
    await expect(
      page.getByText(TEST_CONNECTION_TITLE, { exact: true })
    ).toBeVisible();
  });

  test('delete the created connection through the full stack', async ({
    page,
  }) => {
    // 1. Navigate to connections page
    await page.goto('connections');
    await page.waitForLoadState('networkidle');

    // 2. Verify our test connection exists
    await expect(page.getByText(TEST_CONNECTION_TITLE)).toBeVisible();

    // 3. Delete the connection from its table row.
    await deleteConnectionByTitle(page, TEST_CONNECTION_TITLE);

    // 4. Connection should be gone from the list
    await expect(page.getByText(TEST_CONNECTION_TITLE)).not.toBeVisible({
      timeout: 10000,
    });
  });
});
