import { test, buildSchema } from '../../../fixtures';
import { EditObjectSchemaPage } from '../../../pages/ObjectSchemasPage';

test.describe('Edit object schema (mocked)', () => {
  test('renders with existing schema, a11y + snapshot', async ({
    page,
    mockApi,
    runA11y,
  }) => {
    const schema = buildSchema({ id: 'sch_edit_1', name: 'Customers' });
    await mockApi.bootstrap(page);
    await mockApi.objects.schemas.list(page, [schema]);
    await mockApi.objects.schemas.get(page, schema.id, schema);

    const view = new EditObjectSchemaPage(page, schema.id);
    await view.goto();

    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('objects-schema-edit');
  });
});
