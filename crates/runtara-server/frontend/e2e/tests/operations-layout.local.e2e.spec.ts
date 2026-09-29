import { test, expect } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { appPath } from '../utils/app-path';

// Read-only layout regression against the same isolated fixture as the answer test.
const fixtureFile = process.env.E2E_OPERATIONS_FIXTURE;
for (const width of [1440, 1000, 390]) {
  test(`Operations layout fits ${width}px and keeps Runs actions aligned`, async ({
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
      ['/operations/runs?status=failed,timeout', 'Runs'],
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
      if (title === 'Overview') {
        await expect(
          page.getByRole('heading', { name: 'Processes', exact: true })
        ).toHaveCount(0);
        const requests = page.getByRole('list', {
          name: 'Requests requiring input',
        });
        const failures = page.getByRole('list', { name: 'Recent failures' });
        await expect(requests).toBeVisible();
        await expect(failures).toBeVisible();
        await expect(
          page.getByRole('link', { name: 'View all requests' })
        ).toHaveAttribute('href', appPath('/operations/queues'));
        await expect(
          page.getByRole('link', { name: 'View failed runs' })
        ).toHaveAttribute('href', /dateBasis=completed/);
        let sawNonOverdue = false;
        for (const item of await requests.getByRole('listitem').all()) {
          const overdue =
            (await item.getByText('Overdue', { exact: true }).count()) > 0;
          if (overdue) expect(sawNonOverdue).toBe(false);
          else sawNonOverdue = true;
          const review = item.getByRole('link', {
            name: /^Review request for/,
          });
          await expect(review).toBeVisible();
          if (width >= 1024)
            expect((await item.boundingBox())!.height).toBeLessThan(90);
        }
        for (const item of await failures.getByRole('listitem').all()) {
          const box = (await item.boundingBox())!;
          const actionLinks = item.getByRole('link', {
            name: 'Open execution',
          });
          const copy = item.getByRole('button', { name: 'Copy run ID' });
          const eyeBox = (await actionLinks.boundingBox())!;
          const copyBox = (await copy.boundingBox())!;
          expect(eyeBox.y).toBe(copyBox.y);
          expect(copyBox.x + copyBox.width).toBeLessThanOrEqual(
            box.x + box.width
          );
          if (width >= 1024) expect(box.height).toBeLessThan(90);
        }
        await failures
          .getByRole('button', { name: 'Replay', exact: true })
          .first()
          .click();
        await expect(page.getByRole('dialog')).toContainText(
          'repeats all side effects'
        );
        await page.getByRole('button', { name: 'Cancel', exact: true }).click();
        await expect(page.getByRole('dialog')).toHaveCount(0);
      }
      if (title === 'Runs') {
        const items =
          width >= 1024
            ? page.getByRole('row').filter({
                has: page.getByRole('link', { name: 'Open execution' }),
              })
            : page.getByRole('article');
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
