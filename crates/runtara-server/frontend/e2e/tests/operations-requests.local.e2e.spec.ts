import { test, expect } from '@playwright/test';
import { appPath } from '../utils/app-path';

for (const width of [1440, 390]) {
  test(`Overview request metrics open matching lists at ${width}px`, async ({
    page,
  }) => {
    test.skip(
      !process.env.E2E_OPERATIONS_FIXTURE,
      'Requires the isolated Operations fixture'
    );
    await page.setViewportSize({ width, height: 1000 });
    await page.goto(appPath('/operations'));
    const waiting = page
      .getByText('Waiting for a decision', { exact: true })
      .locator('..');
    const overdue = page
      .getByText('Past their configured due time', { exact: true })
      .locator('../..');
    await expect(waiting.locator('p').nth(1)).toHaveText(/^\d+$/);
    await expect(overdue.locator('p').nth(1)).toHaveText(/^\d+$/);
    const waitingCount = Number(await waiting.locator('p').nth(1).innerText());
    const overdueCount = Number(await overdue.locator('p').nth(1).innerText());
    expect(waitingCount).toBeGreaterThan(overdueCount);
    expect(overdueCount).toBeGreaterThan(0);
    await waiting
      .getByRole('link', { name: 'View requests', exact: true })
      .click();
    await expect(page).toHaveURL(/\/operations\/requests$/);
    await expect(
      page.getByRole('heading', {
        name: `${waitingCount} waiting requests`,
        exact: true,
      })
    ).toBeVisible();
    await expect(
      page
        .getByRole('list', { name: 'Requests requiring input' })
        .getByRole('listitem')
    ).toHaveCount(waitingCount);
    await page.goBack();
    await overdue.getByRole('link', { name: 'Review', exact: true }).click();
    await expect(page).toHaveURL(/\/operations\/requests\?filter=overdue$/);
    await expect(
      page.getByRole('heading', {
        name: `${overdueCount} overdue requests`,
        exact: true,
      })
    ).toBeVisible();
    await expect(
      page
        .getByRole('list', { name: 'Requests requiring input' })
        .getByRole('listitem')
    ).toHaveCount(overdueCount);
    await page.reload();
    await expect(
      page.getByRole('heading', {
        name: `${overdueCount} overdue requests`,
        exact: true,
      })
    ).toBeVisible();
    await expect(
      page
        .getByRole('navigation', { name: 'Request filters' })
        .getByRole('link', { name: 'Overdue', exact: true })
    ).toHaveAttribute('aria-current', 'page');
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth)
    ).toBeLessThanOrEqual(width);
  });
}
