import { test, buildSchema } from '../../../fixtures';
import { CreateObjectInstancePage } from '../../../pages/ObjectSchemasPage';

test.describe('Create object instance (mocked)', () => {
  test('renders form, a11y + snapshot', async ({ page, mockApi, runA11y }) => {
    const schema = buildSchema({ id: 'sch_1', name: 'Customers' });
    await mockApi.bootstrap(page);
    await mockApi.objects.schemas.list(page, [schema]);
    await mockApi.objects.schemas.get(page, schema.id, schema);

    const view = new CreateObjectInstancePage(page, 'Customers');
    await view.goto();

    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('objects-instance-create');
  });
});
