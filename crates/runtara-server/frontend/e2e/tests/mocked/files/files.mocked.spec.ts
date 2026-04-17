import { test, expect } from '../../../fixtures';
import { FilesPage } from '../../../pages/FilesPage';

test.describe('Files page (mocked)', () => {
  test('empty state renders, passes a11y, matches snapshot', async ({
    page,
    mockApi,
    runA11y,
  }) => {
    await mockApi.bootstrap(page);
    await mockApi.connections.list(page, []);
    await mockApi.files.buckets(page, []);

    const files = new FilesPage(page);
    await files.goto();

    await files.expectHeading(/file storage/i);
    await expect(files.emptyStateMessage).toBeVisible();

    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await files.expectMatchesSnapshot('files-empty');
  });
});
