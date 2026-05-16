import {
  test,
  expect,
  buildConnectionType,
  buildObjectModelConnection,
  buildSchema,
} from '../../../fixtures';
import { ObjectSchemasPage } from '../../../pages/ObjectSchemasPage';

test.describe('Object schemas connection selection (mocked)', () => {
  test('uses the default database connection and reloads schemas after switching', async ({
    page,
    mockApi,
  }) => {
    const defaultConnection = buildObjectModelConnection({
      id: 'conn_primary_db',
      title: 'Primary database',
      defaultFor: ['object_model'],
    });
    const archiveConnection = buildObjectModelConnection({
      id: 'conn_archive_db',
      title: 'Archive database',
      defaultFor: [],
    });
    const seenConnectionIds: string[] = [];

    await mockApi.bootstrap(page);
    await mockApi.connections.list(page, [
      defaultConnection,
      archiveConnection,
    ]);
    await mockApi.connections.types(page, [
      buildConnectionType({
        integrationId: 'postgres',
        displayName: 'PostgreSQL',
      }),
    ]);
    await mockApi.raw(
      page,
      /\/api\/runtime(?:\/[^/]+)?\/object-model\/schemas(?:\?[^/]*)?$/,
      async (route) => {
        const url = new URL(route.request().url());
        const connectionId = url.searchParams.get('connectionId') ?? '';
        seenConnectionIds.push(connectionId);
        const schemas =
          connectionId === archiveConnection.id
            ? [buildSchema({ name: 'ArchiveOrders' })]
            : [buildSchema({ name: 'PrimaryOrders' })];

        await route.fulfill({
          status: 200,
          contentType: 'application/json',
          body: JSON.stringify({
            schemas,
            totalCount: schemas.length,
          }),
        });
      }
    );

    const view = new ObjectSchemasPage(page);
    await view.goto();

    await expect(page.getByText('PrimaryOrders')).toBeVisible();
    expect(seenConnectionIds).toContain(defaultConnection.id);

    await page.getByLabel('Database connection').click();
    await page.getByRole('option', { name: /Archive database/ }).click();

    await expect(page).toHaveURL(/connectionId=conn_archive_db/);
    await expect(page.getByText('ArchiveOrders')).toBeVisible();
    expect(seenConnectionIds).toContain(archiveConnection.id);
  });
});
