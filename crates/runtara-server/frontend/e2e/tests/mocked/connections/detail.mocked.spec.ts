import { test, buildConnection, buildConnectionType } from '../../../fixtures';
import { ConnectionDetailPage } from '../../../pages/ConnectionsPage';

test.describe('Connection detail (mocked)', () => {
  test('renders existing connection, a11y + snapshot', async ({
    page,
    mockApi,
    runA11y,
  }) => {
    const conn = buildConnection({
      id: 'conn_detail',
      integrationId: 'http',
      title: 'HTTP Webhook Connection',
    });
    const type = buildConnectionType({
      integrationId: 'http',
      displayName: 'HTTP',
      fields: [
        {
          name: 'url',
          displayName: 'URL',
          isOptional: false,
          isSecret: false,
          typeName: 'String',
        } as any,
      ],
    });

    await mockApi.bootstrap(page);
    await mockApi.connections.list(page, [conn]);
    await mockApi.connections.get(page, conn.id, conn);
    await mockApi.connections.types(page, [type]);

    const view = new ConnectionDetailPage(page, conn.id);
    await view.goto();

    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('connections-detail');
  });
});
