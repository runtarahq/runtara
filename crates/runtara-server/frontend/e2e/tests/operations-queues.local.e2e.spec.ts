import { test, expect } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { appPath } from '../utils/app-path';

for (const width of [1440, 390]) {
  test(`custom queues can be created, edited and deleted at ${width}px`, async ({
    page,
    request,
  }) => {
    test.skip(
      !process.env.E2E_OPERATIONS_FIXTURE,
      'Requires the isolated Operations fixture'
    );
    const fixture = JSON.parse(
      readFileSync(process.env.E2E_OPERATIONS_FIXTURE!, 'utf8')
    );
    const name = `Queue CRUD ${width} ${Date.now()}`;
    let id: string | undefined;
    await page.setViewportSize({ width, height: 1000 });
    try {
      await page.goto(appPath('/operations/queues'));
      await expect(
        page.getByRole('heading', { name: 'Shared views' })
      ).toHaveCount(0);
      await page
        .getByRole('link', { name: 'Create queue', exact: true })
        .click();
      await page
        .getByRole('combobox', { name: 'Workflow', exact: true })
        .selectOption(fixture.workflow);
      await page.getByRole('textbox', { name: 'Name', exact: true }).fill(name);
      await expect(
        page.getByRole('combobox', { name: 'Queue contents' })
      ).not.toHaveValue('');
      await page
        .getByRole('button', { name: 'Create queue', exact: true })
        .click();
      await expect(page).toHaveURL(/\/operations\/queues\/[a-f0-9-]+$/);
      id = new URL(page.url()).pathname.split('/').at(-1)!;
      await expect(
        page.getByRole('heading', { name, exact: true })
      ).toBeVisible();
      await page.getByRole('link', { name: 'Edit queue', exact: true }).click();
      await expect(
        page.getByRole('combobox', { name: 'Workflow', exact: true })
      ).toBeDisabled();
      await page
        .getByRole('textbox', { name: 'Name', exact: true })
        .fill(`${name} edited`);
      await page
        .getByRole('combobox', { name: 'Queue contents' })
        .selectOption('');
      const statuses = page.getByRole('group', { name: 'Run status' });
      await expect(
        statuses.getByRole('button', { name: 'All statuses', exact: true })
      ).toHaveAttribute('aria-pressed', 'true');
      await statuses
        .getByRole('button', { name: 'Completed', exact: true })
        .click();
      await statuses
        .getByRole('button', { name: 'Failed', exact: true })
        .click();
      await page
        .getByRole('button', { name: 'Save changes', exact: true })
        .click();
      await expect(
        page.getByRole('heading', { name: `${name} edited`, exact: true })
      ).toBeVisible();
      await page.reload();
      await expect(
        page.getByRole('heading', { name: `${name} edited`, exact: true })
      ).toBeVisible();
      await page.getByRole('link', { name: 'Edit queue', exact: true }).click();
      await expect(
        statuses.getByRole('button', { name: 'Completed', exact: true })
      ).toHaveAttribute('aria-pressed', 'true');
      await expect(
        statuses.getByRole('button', { name: 'Failed', exact: true })
      ).toHaveAttribute('aria-pressed', 'true');
      await statuses
        .getByRole('button', { name: 'Failed', exact: true })
        .click();
      await expect(
        statuses.getByRole('button', { name: 'Failed', exact: true })
      ).toHaveAttribute('aria-pressed', 'false');
      await statuses
        .getByRole('button', { name: 'All statuses', exact: true })
        .click();
      await expect(
        statuses.getByRole('button', { name: 'Completed', exact: true })
      ).toHaveAttribute('aria-pressed', 'false');
      await expect(
        statuses.getByRole('button', { name: 'All statuses', exact: true })
      ).toHaveAttribute('aria-pressed', 'true');
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth)
      ).toBeLessThanOrEqual(width);
      await page.getByRole('button', { name: 'Cancel', exact: true }).click();
      await page.goto(appPath('/operations/queues'));
      const item = page.getByRole('listitem').filter({
        has: page.getByRole('heading', {
          name: `${name} edited`,
          exact: true,
        }),
      });
      await expect(item).toBeVisible();
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth)
      ).toBeLessThanOrEqual(width);
      await item
        .getByRole('button', {
          name: `Delete queue ${name} edited`,
          exact: true,
        })
        .click();
      await page
        .getByRole('dialog')
        .getByRole('button', { name: 'Cancel', exact: true })
        .click();
      await expect(item).toBeVisible();
      await item
        .getByRole('button', {
          name: `Delete queue ${name} edited`,
          exact: true,
        })
        .click();
      await page
        .getByRole('dialog')
        .getByRole('button', { name: 'Delete queue', exact: true })
        .click();
      await expect(item).toHaveCount(0);
      await page.reload();
      await expect(
        page.getByRole('heading', { name: `${name} edited`, exact: true })
      ).toHaveCount(0);
      id = undefined;
    } finally {
      // Clean up only this test's explicitly created queue if an assertion failed.
      if (id) {
        const response = await request.get(
          `${fixture.baseUrl}/api/runtime/operations/views`
        );
        const saved = (await response.json()).data.find(
          (view: { id: string }) => view.id === id
        );
        if (saved)
          await request.delete(
            `${fixture.baseUrl}/api/runtime/operations/views/${id}?revision=${saved.revision}`
          );
      }
    }
  });
}
