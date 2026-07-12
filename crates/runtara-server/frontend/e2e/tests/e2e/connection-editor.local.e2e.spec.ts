import { expect, test } from '@playwright/test';

const runId = Date.now();
const originalTitle = `E2E Schema Form SFTP ${runId}`;
const updatedTitle = `${originalTitle} updated`;

test.describe.serial('Connection schema form local UI', () => {
  test.afterEach(async ({ page }) => {
    await page.goto('/connections', { waitUntil: 'domcontentloaded' });
    await expect(
      page.getByRole('button', { name: 'New connection' })
    ).toBeVisible();

    for (const title of [updatedTitle, originalTitle]) {
      const row = page.locator('tr').filter({ hasText: title });
      if ((await row.count()) !== 1) continue;
      await row.getByTitle('Delete connection').click();
      await page.getByRole('button', { name: 'Delete connection' }).click();
      await expect(row).not.toBeVisible();
    }
  });

  test('renders, edits, and safely preserves a schema-defined secret', async ({
    page,
  }) => {
    await page.goto('/connections', { waitUntil: 'domcontentloaded' });

    const newConnection = page.getByRole('button', {
      name: 'New connection',
    });
    await expect(newConnection).toBeVisible();
    await newConnection.click();
    const picker = page.getByRole('dialog');
    await expect(picker).toBeVisible();
    await picker.getByText('SFTP', { exact: true }).click();
    await expect(page).toHaveURL(/\/connections\/sftp\/create$/);
    const passwordField = page.locator(
      '[data-field="password"] input[type="password"]'
    );

    await page.getByLabel('Title').fill(originalTitle);
    await page.getByLabel('Host').fill('sftp.example.com');
    await page.getByLabel('Username').fill('schema-form-user');
    await expect(passwordField).toBeVisible();
    await expect(page.getByLabel('Private Key')).not.toBeVisible();
    await expect(page.getByLabel('Passphrase')).not.toBeVisible();
    await page.getByLabel('Authentication Mode').click();
    await page.getByRole('option', { name: 'Private Key' }).click();
    await expect(passwordField).not.toBeVisible();
    await expect(page.getByLabel('Private Key')).toHaveJSProperty(
      'tagName',
      'TEXTAREA'
    );
    await expect(page.getByLabel('Passphrase')).toBeVisible();

    await page.getByLabel('Authentication Mode').click();
    await page.getByRole('option', { name: 'Password' }).click();
    await expect(page.getByLabel('Port')).toHaveValue('22');

    // A canonical conditional requirement is focusable at the submit boundary;
    // ordinary typing/validation never steals focus before this click.
    await page.getByRole('button', { name: 'Create connection' }).click();
    await expect(passwordField).toBeFocused();
    await passwordField.fill('not-a-real-password');

    await page.getByRole('button', { name: 'Create connection' }).click();
    await expect(page).toHaveURL('/connections');
    await expect(page.getByText(originalTitle, { exact: true })).toBeVisible();

    const createdRow = page.locator('tr').filter({ hasText: originalTitle });
    await expect(createdRow).toHaveCount(1);
    await createdRow.getByTitle('Edit connection').click();
    await expect(
      page.getByRole('heading', { name: 'Edit connection' })
    ).toBeVisible();

    await expect(page.getByLabel('Host')).toHaveValue('sftp.example.com');
    await expect(passwordField).toHaveValue('');
    await expect(
      page.getByText(
        'A secret is configured. Enter a value only to replace it.',
        { exact: true }
      )
    ).toBeVisible();

    await page.getByLabel('Title').fill(updatedTitle);
    await page.getByRole('button', { name: 'Save changes' }).click();
    await expect(page).toHaveURL('/connections');
    await expect(page.getByText(updatedTitle, { exact: true })).toBeVisible();

    const updatedRow = page.locator('tr').filter({ hasText: updatedTitle });
    await expect(updatedRow).toHaveCount(1);
    await updatedRow.getByTitle('Edit connection').click();
    await expect(passwordField).toHaveValue('');
    await expect(
      page.getByText(
        'A secret is configured. Enter a value only to replace it.',
        { exact: true }
      )
    ).toBeVisible();

    await page.getByRole('button', { name: 'Clear stored Password' }).click();
    await expect(
      page.getByText('The stored secret will be cleared when you save.')
    ).toBeVisible();
    await page.getByLabel('Authentication Mode').click();
    await page.getByRole('option', { name: 'Private Key' }).click();
    await page.getByLabel('Private Key').fill('not-a-real-private-key');
    await page.getByRole('button', { name: 'Save changes' }).click();
    await expect(page).toHaveURL('/connections');

    await page
      .locator('tr')
      .filter({ hasText: updatedTitle })
      .getByTitle('Edit connection')
      .click();
    await expect(page.getByLabel('Authentication Mode')).toContainText(
      'Private Key'
    );
    await page.getByLabel('Authentication Mode').click();
    await page.getByRole('option', { name: 'Password' }).click();
    await expect(page.getByText('No secret is configured.')).toBeVisible();
  });

  test('renders MCP authentication modes from canonical conditions', async ({
    page,
  }) => {
    await page.goto('/connections', { waitUntil: 'domcontentloaded' });
    await page.getByRole('button', { name: 'New connection' }).click();
    const picker = page.getByRole('dialog');
    await picker.getByText('MCP Server', { exact: true }).click();

    await expect(page.getByLabel('Auth Mode')).toContainText('None');
    await expect(page.getByLabel('Bearer Token')).not.toBeVisible();
    await expect(page.getByLabel('API Key Header')).not.toBeVisible();
    await expect(page.getByLabel('API Key')).not.toBeVisible();

    await page.getByLabel('Auth Mode').click();
    await page.getByRole('option', { name: 'Bearer' }).click();
    await expect(page.getByLabel('Bearer Token*')).toBeVisible();
    await expect(page.getByLabel('API Key')).not.toBeVisible();

    await page.getByLabel('Auth Mode').click();
    await page.getByRole('option', { name: 'Api Key' }).click();
    await expect(page.getByLabel('Bearer Token')).not.toBeVisible();
    await expect(page.getByLabel('API Key Header')).toBeVisible();
    await expect(page.getByLabel('API Key*')).toBeVisible();
  });
});
