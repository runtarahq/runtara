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

    const row = page.locator('tr').filter({ hasText: updatedTitle });
    if ((await row.count()) !== 1) return;

    await row.getByTitle('Delete connection').click();
    await page.getByRole('button', { name: 'Delete connection' }).click();
    await expect(row).not.toBeVisible();
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

    await page.getByLabel('Title').fill(originalTitle);
    await page.getByLabel('Host').fill('sftp.example.com');
    await page.getByLabel('Username').fill('schema-form-user');
    await page.getByLabel('Password').fill('not-a-real-password');

    const privateKey = page.getByLabel('Private Key');
    await expect(privateKey).toHaveJSProperty('tagName', 'TEXTAREA');
    await expect(page.getByLabel('Port')).toHaveValue('22');

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
    await expect(page.getByLabel('Password')).toHaveValue('');
    await expect(
      page.getByText(
        'A secret is configured. Enter a new value only to replace it.',
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
    await expect(page.getByLabel('Password')).toHaveValue('');
    await expect(
      page.getByText(
        'A secret is configured. Enter a new value only to replace it.',
        { exact: true }
      )
    ).toBeVisible();
  });
});
