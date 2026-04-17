import { test, buildSchema } from '../../../fixtures';
import { ObjectSchemasPage } from '../../../pages/ObjectSchemasPage';

test.describe('Object schemas list (mocked)', () => {
  test('renders page with schemas, a11y + snapshot', async ({
    page,
    mockApi,
    runA11y,
  }) => {
    await mockApi.bootstrap(page);
    await mockApi.objects.schemas.list(page, [
      buildSchema({ name: 'Customers' }),
      buildSchema({ name: 'Orders' }),
    ]);

    const view = new ObjectSchemasPage(page);
    await view.goto();

    await view.expectHeading(/object types/i);
    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('objects-schemas-list');
  });

  test('empty state', async ({ page, mockApi, runA11y }) => {
    await mockApi.bootstrap(page);
    await mockApi.objects.schemas.list(page, []);

    const view = new ObjectSchemasPage(page);
    await view.goto();

    await view.expectHeading(/object types/i);
    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('objects-schemas-empty');
  });
});
