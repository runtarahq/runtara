import { test, buildSchema, buildInstance } from '../../../fixtures';
import { EditObjectInstancePage } from '../../../pages/ObjectSchemasPage';

test.describe('Edit object instance (mocked)', () => {
  test('renders populated form, a11y + snapshot', async ({
    page,
    mockApi,
    runA11y,
  }) => {
    const schema = buildSchema({ id: 'sch_1', name: 'Customers' });
    const instance = buildInstance(schema.id, {
      id: 'inst_edit',
      schemaName: 'Customers',
      properties: { id: 'row_1', status: 'active' },
    });

    await mockApi.bootstrap(page);
    await mockApi.objects.schemas.list(page, [schema]);
    await mockApi.objects.schemas.get(page, schema.id, schema);
    await mockApi.objects.instances.get(page, schema.id, instance.id, instance);

    const view = new EditObjectInstancePage(page, 'Customers', instance.id);
    await view.goto();

    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('objects-instance-edit');
  });
});
