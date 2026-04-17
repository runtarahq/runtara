import { test, expect } from '../../../fixtures';
import { CreateObjectSchemaPage } from '../../../pages/ObjectSchemasPage';

test.describe('Create object schema (mocked)', () => {
  test('renders, a11y + snapshot', async ({ page, mockApi, runA11y }) => {
    await mockApi.bootstrap(page);
    await mockApi.objects.schemas.list(page, []);

    const view = new CreateObjectSchemaPage(page);
    await view.goto();

    await expect(page.getByText(/objects/i).first()).toBeVisible();
    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('objects-schema-create');
  });
});
