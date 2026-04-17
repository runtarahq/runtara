import { test, buildConnectionType } from '../../../fixtures';
import { CreateConnectionPage } from '../../../pages/ConnectionsPage';

test.describe('Create connection (mocked)', () => {
  test('renders form for integration type, a11y + snapshot', async ({
    page,
    mockApi,
    runA11y,
  }) => {
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
    await mockApi.connections.list(page, []);
    await mockApi.connections.types(page, [type]);
    await mockApi.connections.typeById(page, 'http', type);

    const view = new CreateConnectionPage(page, 'http');
    await view.goto();

    await runA11y(page, { exclude: ['[data-sonner-toaster]'] });
    await view.expectMatchesSnapshot('connections-create');
  });
});
