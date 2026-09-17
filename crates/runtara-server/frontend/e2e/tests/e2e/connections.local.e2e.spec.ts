import { test, expect } from '@playwright/test';

const apiBase = `${process.env.E2E_RUNTIME_URL || 'http://127.0.0.1:7001'}/api/runtime`;
const title = `E2E connection catalog MCP ${Date.now()}`;
let connectionId: string;

test.describe('Connection catalog without native agents', () => {
  test.beforeAll(async ({ request }) => {
    const response = await request.post(`${apiBase}/connections`, {
      data: {
        title,
        integrationId: 'mcp',
        connectionParameters: {
          url: 'https://mcp.example.com',
          auth_mode: 'none',
        },
      },
    });
    expect(response.status()).toBe(201);
    connectionId = (await response.json()).connectionId;
  });

  test.afterAll(async ({ request }) => {
    if (connectionId) {
      const response = await request.delete(
        `${apiBase}/connections/${connectionId}`
      );
      expect(response.status()).toBe(200);
    }
  });

  test('lists a retained integration and offers supported connection types', async ({
    page,
  }) => {
    await page.goto('connections');
    const row = page.locator('tr').filter({ hasText: title });
    await expect(row).toHaveCount(1);
    await expect(row).toContainText('MCP');
    await page.getByRole('button', { name: 'New connection' }).click();
    const picker = page.getByRole('dialog');
    await expect(picker.getByText('SFTP', { exact: true })).toHaveCount(0);
    await expect(picker.getByText('MCP Server', { exact: true })).toBeVisible();
    await expect(
      picker.getByText('PostgreSQL Database', { exact: true })
    ).toBeVisible();
  });

  test('search filters supported connection types', async ({ page }) => {
    await page.goto('connections');
    await page.getByRole('button', { name: 'New connection' }).click();
    const picker = page.getByRole('dialog');
    await picker.getByPlaceholder(/search/i).fill('MCP');
    await expect(picker.getByText('MCP Server', { exact: true })).toBeVisible();
    await expect(
      picker.getByText('PostgreSQL Database', { exact: true })
    ).toHaveCount(0);
  });
});
