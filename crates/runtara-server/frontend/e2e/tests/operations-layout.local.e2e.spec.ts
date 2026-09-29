import { test, expect } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { appPath } from '../utils/app-path';

// Read-only layout regression against the same isolated fixture as the answer test.
const fixtureFile = process.env.E2E_OPERATIONS_FIXTURE;
for (const width of [1440, 1000, 390]) {
  test(`Operations layout fits ${width}px and keeps Monitor actions aligned`, async ({
    page,
  }, testInfo) => {
    test.skip(
      !fixtureFile,
      'Set E2E_OPERATIONS_FIXTURE to an isolated live fixture'
    );
    const fixture = JSON.parse(readFileSync(fixtureFile!, 'utf8')) as {
      view: string;
      workflow: string;
      runs: string[];
    };
    await page.setViewportSize({ width, height: 1000 });
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(e.message));
    page.on('response', (r) => {
      if (r.url().includes('/api/') && r.status() >= 400)
        errors.push(`${r.status()} ${new URL(r.url()).pathname}`);
    });
    for (const [route, title] of [
      ['/operations', 'Overview'],
      ['/operations/monitor', 'Monitor'],
      [`/operations/views/${fixture.view}`, null],
      [`/operations/runs/${fixture.workflow}/${fixture.runs[0]}`, 'ORDER-123'],
    ] as const) {
      await page.goto(appPath(route));
      await expect(
        title
          ? page.getByRole('heading', { name: title, exact: true })
          : page.getByRole('heading', { level: 1 })
      ).toBeVisible();
      await page.waitForLoadState('networkidle');
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth)
      ).toBeLessThanOrEqual(width);
      if (title === 'Monitor') {
        await expect(
          page.getByRole('table', { name: 'Process health' })
        ).toBeVisible();
        const items = page.locator('[data-run-id]');
        expect(await items.count()).toBeGreaterThan(0);
        for (const item of await items.all()) {
          const button = item.getByRole('link', { name: 'Open execution' });
          const rowBox = await item.boundingBox();
          const buttonBox = await button.boundingBox();
          expect(buttonBox!.x + buttonBox!.width).toBeLessThanOrEqual(
            rowBox!.x + rowBox!.width
          );
          expect(buttonBox!.y + buttonBox!.height).toBeLessThanOrEqual(
            rowBox!.y + rowBox!.height
          );
        }
      }
      await page.screenshot({
        path: testInfo.outputPath(`${title ?? 'Queue'}.png`),
        fullPage: true,
      });
    }
    expect(errors).toEqual([]);
  });
}
