import { test, buildSchema, buildInstance } from '../../../fixtures';
import { ManageInstancesPage } from '../../../pages/ObjectSchemasPage';

test.describe('Object instances list (mocked)', () => {
  test('renders instances, a11y + snapshot', async ({
    page,
    mockApi,
    runA11y,
  }) => {
    const schema = buildSchema({ id: 'sch_1', name: 'Customers' });
    const instances = [
      buildInstance(schema.id, {
        id: 'inst_1',
        schemaName: 'Customers',
        properties: { id: '1' },
      }),
      buildInstance(schema.id, {
        id: 'inst_2',
        schemaName: 'Customers',
        properties: { id: '2' },
      }),
    ];

    await mockApi.bootstrap(page);
    await mockApi.objects.schemas.list(page, [schema]);
    await mockApi.objects.schemas.get(page, schema.id, schema);
    await mockApi.objects.instances.listBySchemaId(page, schema.id, instances);
    await mockApi.objects.instances.filterBySchemaName(
      page,
      'Customers',
      instances
    );

    const view = new ManageInstancesPage(page, 'Customers');
    await view.goto();

    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('objects-instances-list');
  });
});
