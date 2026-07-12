import {
  expect,
  test,
  type APIRequestContext,
  type Page,
} from '@playwright/test';

const runId = Date.now();
const originalTitle = `E2E Schema Form SFTP ${runId}`;
const updatedTitle = `${originalTitle} updated`;
const apiBase = 'http://127.0.0.1:7001/api/runtime';
const apiConnectionIds = new Set<string>();

type ApiConnection = {
  id: string;
  title: string;
  status?: string;
  editProjection: {
    values: Record<string, unknown>;
    secretState: Record<string, { configured: boolean; clearable: boolean }>;
    version: string;
  };
};

async function createApiConnection(
  request: APIRequestContext,
  body: Record<string, unknown>
): Promise<string> {
  const response = await request.post(`${apiBase}/connections`, { data: body });
  expect(response.status(), await response.text()).toBe(201);
  const payload = (await response.json()) as { connectionId: string };
  apiConnectionIds.add(payload.connectionId);
  return payload.connectionId;
}

async function getApiConnection(
  request: APIRequestContext,
  id: string
): Promise<ApiConnection> {
  const response = await request.get(`${apiBase}/connections/${id}`);
  expect(response.status(), await response.text()).toBe(200);
  return ((await response.json()) as { connection: ApiConnection }).connection;
}

async function updateApiConnection(
  request: APIRequestContext,
  id: string,
  body: Record<string, unknown>
) {
  return request.put(`${apiBase}/connections/${id}`, { data: body });
}

async function openConnectionEditor(page: Page, title: string) {
  await page.goto('/connections', { waitUntil: 'domcontentloaded' });
  const row = page.locator('tr').filter({ hasText: title });
  await expect(row).toHaveCount(1);
  await row.getByTitle('Edit connection').click();
  await expect(page.getByRole('heading', { name: title })).toBeVisible();
}

test.describe.serial('Connection schema form local UI', () => {
  test.afterEach(async ({ page, request }) => {
    await page.goto('/connections', { waitUntil: 'domcontentloaded' });
    await expect(
      page.getByRole('button', { name: 'New connection' })
    ).toBeVisible();

    for (const title of [updatedTitle, originalTitle]) {
      const row = page.locator('tr').filter({ hasText: title });
      if ((await row.count()) !== 1) continue;
      await row.getByTitle('Delete connection').click();
      await page.getByRole('button', { name: 'Delete connection' }).click();
      await expect(row).not.toBeVisible();
    }

    for (const id of apiConnectionIds) {
      const response = await request.delete(`${apiBase}/connections/${id}`);
      expect([200, 404]).toContain(response.status());
      apiConnectionIds.delete(id);
    }
  });

  test('renders, edits, and safely preserves a schema-defined secret', async ({
    page,
  }) => {
    await page.goto('/connections', { waitUntil: 'domcontentloaded' });

    const newConnection = page.getByRole('button', {
      name: 'New connection',
    });
    await expect(newConnection).toBeVisible();
    await newConnection.click();
    const picker = page.getByRole('dialog');
    await expect(picker).toBeVisible();
    await picker.getByText('SFTP', { exact: true }).click();
    await expect(page).toHaveURL(/\/connections\/sftp\/create$/);
    const passwordField = page.locator(
      '[data-field="password"] input[type="password"]'
    );

    await page.getByLabel('Title').fill(originalTitle);
    await page.getByLabel('Host').fill('sftp.example.com');
    await page.getByLabel('Username').fill('schema-form-user');
    await expect(passwordField).toBeVisible();
    await expect(page.getByLabel('Private Key')).not.toBeVisible();
    await expect(page.getByLabel('Passphrase')).not.toBeVisible();
    await page.getByLabel('Authentication Mode').click();
    await page.getByRole('option', { name: 'Private Key' }).click();
    await expect(passwordField).not.toBeVisible();
    await expect(page.getByLabel('Private Key')).toHaveJSProperty(
      'tagName',
      'TEXTAREA'
    );
    await expect(page.getByLabel('Passphrase')).toBeVisible();

    await page.getByLabel('Authentication Mode').click();
    await page.getByRole('option', { name: 'Password' }).click();
    await expect(page.getByLabel('Port')).toHaveValue('22');

    // A canonical conditional requirement is focusable at the submit boundary;
    // ordinary typing/validation never steals focus before this click.
    await page.getByRole('button', { name: 'Create connection' }).click();
    await expect(passwordField).toBeFocused();
    await passwordField.fill('not-a-real-password');

    await page.getByRole('button', { name: 'Create connection' }).click();
    await expect(page).toHaveURL('/connections');
    await expect(page.getByText(originalTitle, { exact: true })).toBeVisible();

    const createdRow = page.locator('tr').filter({ hasText: originalTitle });
    await expect(createdRow).toHaveCount(1);
    await createdRow.getByTitle('Edit connection').click();
    await expect(
      page.getByRole('heading', { name: originalTitle })
    ).toBeVisible();

    await expect(page.getByLabel('Host')).toHaveValue('sftp.example.com');
    await expect(passwordField).toHaveValue('');
    await expect(
      page.getByText(
        'A secret is configured. Enter a value only to replace it.',
        { exact: true }
      )
    ).toBeVisible();

    await page.getByLabel('Title').fill(updatedTitle);
    // The header renames live from the Title field.
    await expect(
      page.getByRole('heading', { name: updatedTitle })
    ).toBeVisible();
    await page.getByRole('button', { name: 'Save changes' }).click();
    // Save stays on the page; the save bar collapses once the write lands.
    await expect(page.getByText('Connection saved.').first()).toBeVisible();
    await expect(
      page.getByRole('button', { name: 'Save changes' })
    ).toBeHidden();

    await page.goto('/connections', { waitUntil: 'domcontentloaded' });
    const updatedRow = page.locator('tr').filter({ hasText: updatedTitle });
    await expect(updatedRow).toHaveCount(1);
    await updatedRow.getByTitle('Edit connection').click();
    await expect(passwordField).toHaveValue('');
    await expect(
      page.getByText(
        'A secret is configured. Enter a value only to replace it.',
        { exact: true }
      )
    ).toBeVisible();

    await page.getByRole('button', { name: 'Clear stored Password' }).click();
    await expect(
      page.getByText('The stored secret will be cleared when you save.')
    ).toBeVisible();
    await page.getByLabel('Authentication Mode').click();
    await page.getByRole('option', { name: 'Private Key' }).click();
    await page.getByLabel('Private Key').fill('not-a-real-private-key');
    await page.getByRole('button', { name: 'Save changes' }).click();
    await expect(page.getByText('Connection saved.').first()).toBeVisible();

    await page.goto('/connections', { waitUntil: 'domcontentloaded' });
    await page
      .locator('tr')
      .filter({ hasText: updatedTitle })
      .getByTitle('Edit connection')
      .click();
    await expect(page.getByLabel('Authentication Mode')).toContainText(
      'Private Key'
    );
    await page.getByLabel('Authentication Mode').click();
    await page.getByRole('option', { name: 'Password' }).click();
    await expect(page.getByText('No secret is configured.')).toBeVisible();
  });

  test('renders MCP authentication modes from canonical conditions', async ({
    page,
  }) => {
    await page.goto('/connections', { waitUntil: 'domcontentloaded' });
    await page.getByRole('button', { name: 'New connection' }).click();
    const picker = page.getByRole('dialog');
    await picker.getByText('MCP Server', { exact: true }).click();

    await expect(page.getByLabel('Auth Mode')).toContainText('None');
    await expect(page.getByLabel('Bearer Token')).not.toBeVisible();
    await expect(page.getByLabel('API Key Header')).not.toBeVisible();
    await expect(page.getByLabel('API Key')).not.toBeVisible();

    await page.getByLabel('Auth Mode').click();
    await page.getByRole('option', { name: 'Bearer' }).click();
    await expect(page.getByLabel('Bearer Token*')).toBeVisible();
    await expect(page.getByLabel('API Key')).not.toBeVisible();

    await page.getByLabel('Auth Mode').click();
    await page.getByRole('option', { name: 'Api Key' }).click();
    await expect(page.getByLabel('Bearer Token')).not.toBeVisible();
    await expect(page.getByLabel('API Key Header')).toBeVisible();
    await expect(page.getByLabel('API Key*')).toBeVisible();
  });

  test('preserves descriptor order and exposes authored advanced sections', async ({
    page,
  }) => {
    await page.goto('/connections', { waitUntil: 'domcontentloaded' });
    await page.getByRole('button', { name: 'New connection' }).click();
    await page.getByRole('dialog').getByText('SFTP', { exact: true }).click();
    await expect(page.getByLabel('Host')).toBeVisible();

    const sftpOrder = await page
      .locator('[data-field]')
      .evaluateAll((nodes) =>
        nodes.map((node) => node.getAttribute('data-field'))
      );
    expect(sftpOrder.slice(0, 5)).toEqual([
      'title',
      'host',
      'port',
      'username',
      'auth_mode',
    ]);

    await page.goto('/connections', { waitUntil: 'domcontentloaded' });
    await page.getByRole('button', { name: 'New connection' }).click();
    await page
      .getByRole('dialog')
      .getByText('QuickBooks Online', { exact: true })
      .click();
    await expect(page.getByLabel('Client ID')).toBeVisible();
    const quickBooksOrder = await page
      .locator('[data-field]')
      .evaluateAll((nodes) =>
        nodes.map((node) => node.getAttribute('data-field'))
      );
    expect(quickBooksOrder.indexOf('client_id')).toBeLessThan(
      quickBooksOrder.indexOf('client_secret')
    );

    await page.goto('/connections', { waitUntil: 'domcontentloaded' });
    await page.getByRole('button', { name: 'New connection' }).click();
    await page
      .getByRole('dialog')
      .getByText('MCP Server', { exact: true })
      .click();
    await expect(page).toHaveURL(/\/connections\/mcp\/create$/);
    await expect(page.getByLabel('Server URL')).toBeVisible();

    const advanced = page
      .locator('details')
      .filter({ hasText: 'Advanced settings' });
    await expect(advanced).toHaveCount(1);
    await expect(advanced).not.toHaveAttribute('open', '');
    await expect(page.getByLabel('Extra Headers')).not.toBeVisible();
    await advanced.locator('summary').focus();
    await page.keyboard.press('Enter');
    await expect(advanced).toHaveAttribute('open', '');
    await expect(page.getByLabel('Extra Headers')).toBeVisible();

    const extraHeaders = page.locator('[data-field="extra_headers"]');
    await extraHeaders.getByPlaceholder('Key').fill('X-E2E');
    await extraHeaders.getByPlaceholder('Value').fill('retained');
    await extraHeaders.getByRole('button', { name: 'Add entry' }).click();
    await advanced.locator('summary').click();
    await expect(page.getByLabel('Extra Headers')).not.toBeVisible();
    await advanced.locator('summary').click();
    await expect(extraHeaders.locator('input[value="X-E2E"]')).toBeVisible();
    await expect(extraHeaders.locator('input[value="retained"]')).toBeVisible();
  });

  test('rejects the legacy update contract at the HTTP boundary', async ({
    request,
  }) => {
    const id = await createApiConnection(request, {
      title: `E2E API contract ${runId}`,
      integrationId: 'quickbooks_online',
      connectionParameters: {
        client_id: 'client-id',
        client_secret: 'stored-secret',
      },
    });
    const opened = await getApiConnection(request, id);

    for (const unsafeCreate of [
      {
        title: `E2E unsafe managed create ${runId}`,
        integrationId: 'quickbooks_online',
        connectionParameters: {
          client_id: 'client-id',
          client_secret: 'stored-secret',
          realm_id: 'server-managed',
        },
      },
      {
        title: `E2E unsafe status create ${runId}`,
        integrationId: 'quickbooks_online',
        status: 'ACTIVE',
        connectionParameters: {
          client_id: 'client-id',
          client_secret: 'stored-secret',
        },
      },
    ]) {
      const response = await request.post(`${apiBase}/connections`, {
        data: unsafeCreate,
      });
      expect([400, 422]).toContain(response.status());
    }

    const missingVersion = await updateApiConnection(request, id, {
      title: 'must not be accepted',
    });
    expect(missingVersion.status()).toBe(422);

    const legacyReplace = await updateApiConnection(request, id, {
      version: opened.editProjection.version,
      connectionParameters: { realm_id: 'forbidden' },
    });
    expect(legacyReplace.status()).toBe(422);

    const managedWrite = await updateApiConnection(request, id, {
      version: opened.editProjection.version,
      connectionParameterPatch: {
        set: { realm_id: 'forbidden' },
        write: {},
        clear: [],
      },
    });
    expect(managedWrite.status()).toBe(400);
    await expect(managedWrite.json()).resolves.toMatchObject({
      success: false,
      message: expect.stringContaining('realm_id'),
    });

    const safeUpdate = await updateApiConnection(request, id, {
      version: opened.editProjection.version,
      title: `E2E API contract ${runId} safe`,
    });
    expect(safeUpdate.status(), await safeUpdate.text()).toBe(200);

    for (const staleBody of [
      {
        version: opened.editProjection.version,
        title: 'stale title must not win',
      },
      {
        version: opened.editProjection.version,
        connectionParameterPatch: {
          set: { environment: 'production' },
          write: {},
          clear: [],
        },
      },
    ]) {
      const stale = await updateApiConnection(request, id, staleBody);
      expect(stale.status()).toBe(409);
      await expect(stale.json()).resolves.toMatchObject({
        message: expect.stringContaining('changed since it was opened'),
      });
    }
  });

  test('title-only UI save preserves an API-created OAuth row with absent defaults', async ({
    page,
    request,
  }) => {
    const title = `E2E legacy QuickBooks ${runId}`;
    const renamed = `${title} renamed`;
    const id = await createApiConnection(request, {
      title,
      integrationId: 'quickbooks_online',
      connectionParameters: {
        client_id: 'client-id',
        client_secret: 'stored-secret',
      },
    });

    await openConnectionEditor(page, title);
    await expect(page.getByLabel('Environment')).toContainText('Sandbox');
    await expect(page.getByLabel('Scopes')).toHaveValue(
      'com.intuit.quickbooks.accounting'
    );

    // The status card replaces the amber reconnect banner: an unauthorized
    // OAuth row shows the "Reconnect required" pill + never-authorized copy
    // and a Connect action, and no role="alert" banner remains.
    await expect(page.getByText('Reconnect required')).toBeVisible();
    await expect(
      page.getByText("This connection isn't authorized", { exact: false })
    ).toBeVisible();
    await expect(page.getByRole('button', { name: 'Connect' })).toBeVisible();
    await expect(page.getByRole('alert')).toHaveCount(0);

    let submittedBody: Record<string, unknown> | undefined;
    page.on('request', (outgoing) => {
      if (
        outgoing.method() === 'PUT' &&
        outgoing.url().endsWith(`/api/runtime/connections/${id}`)
      ) {
        submittedBody = outgoing.postDataJSON() as Record<string, unknown>;
      }
    });
    await page.getByLabel('Title').fill(renamed);
    await page.getByRole('button', { name: 'Save changes' }).click();
    await expect(page.getByText('Connection saved.').first()).toBeVisible();

    expect(submittedBody).toMatchObject({ title: renamed });
    expect(submittedBody).not.toHaveProperty('connectionParameterPatch');
    const saved = await getApiConnection(request, id);
    expect(saved.status).toBe('REQUIRES_RECONNECTION');
    expect(saved.editProjection.secretState.client_secret.configured).toBe(
      true
    );
    expect(saved.editProjection.values).not.toHaveProperty('environment');
    expect(saved.editProjection.values).not.toHaveProperty('scopes');
  });

  test('keeps a stale draft and explicitly reapplies it to the latest version', async ({
    page,
    request,
  }) => {
    const title = `E2E conflict SFTP ${runId}`;
    const serverTitle = `${title} server`;
    const id = await createApiConnection(request, {
      title,
      integrationId: 'sftp',
      connectionParameters: {
        host: 'old.example.com',
        port: 22,
        username: 'conflict-user',
        auth_mode: 'password',
        password: 'stored-secret',
      },
    });

    await openConnectionEditor(page, title);
    const opened = await getApiConnection(request, id);
    const concurrent = await updateApiConnection(request, id, {
      version: opened.editProjection.version,
      title: serverTitle,
    });
    expect(concurrent.status(), await concurrent.text()).toBe(200);

    await page.getByLabel('Host').fill('draft.example.com');
    await page.getByRole('button', { name: 'Save changes' }).click();
    const notice = page.getByRole('alert').filter({
      hasText: 'Review newer connection changes',
    });
    await expect(notice).toBeVisible();
    await expect(notice).toContainText('Changed on the server: Title.');
    await expect(page.getByLabel('Host')).toHaveValue('draft.example.com');

    await notice
      .getByRole('button', { name: 'Apply my submitted changes' })
      .click();
    await expect(page.getByText('Connection saved.').first()).toBeVisible();
    const reapplied = await getApiConnection(request, id);
    expect(reapplied.title).toBe(serverTitle);
    expect(reapplied.editProjection.values.host).toBe('draft.example.com');
    expect(reapplied.editProjection.secretState.password.configured).toBe(true);
  });

  test('save bar tracks dirty state, discards edits, and guards navigation', async ({
    page,
    request,
  }) => {
    const title = `E2E save bar SFTP ${runId}`;
    const id = await createApiConnection(request, {
      title,
      integrationId: 'sftp',
      connectionParameters: {
        host: 'bar.example.com',
        port: 22,
        username: 'bar-user',
        auth_mode: 'password',
        password: 'stored-secret',
      },
    });

    await openConnectionEditor(page, title);

    const saveButton = page.getByRole('button', { name: 'Save changes' });
    const discardButton = page.getByRole('button', { name: 'Discard' });

    // Pristine: no save bar.
    await expect(saveButton).toBeHidden();
    await expect(discardButton).toBeHidden();

    // Editing a field surfaces the save bar with a dirty summary.
    await page.getByLabel('Host').fill('edited.example.com');
    await expect(saveButton).toBeVisible();
    await expect(page.getByText('1 unsaved change')).toBeVisible();

    // Discard reverts the field to the stored value and hides the bar.
    await discardButton.click();
    await expect(page.getByLabel('Host')).toHaveValue('bar.example.com');
    await expect(saveButton).toBeHidden();

    // Navigating away while dirty prompts the unsaved-changes guard.
    await page.getByLabel('Host').fill('dirty.example.com');
    await page.getByRole('link', { name: 'Back to connections' }).click();
    const dialog = page.getByRole('alertdialog');
    await expect(dialog).toContainText('Unsaved changes');
    await dialog.getByRole('button', { name: 'Keep editing' }).click();
    await expect(page.getByLabel('Host')).toHaveValue('dirty.example.com');

    // Discarding through the guard leaves the page.
    await page.getByRole('link', { name: 'Back to connections' }).click();
    await page
      .getByRole('alertdialog')
      .getByRole('button', { name: 'Discard changes' })
      .click();
    await expect(page).toHaveURL('/connections');

    // The discarded edit never reached the server.
    const saved = await getApiConnection(request, id);
    expect(saved.editProjection.values.host).toBe('bar.example.com');
  });

  test('guards reconnect when unsaved credential changes would be ignored', async ({
    page,
    request,
  }) => {
    const title = `E2E reconnect guard QBO ${runId}`;
    const id = await createApiConnection(request, {
      title,
      integrationId: 'quickbooks_online',
      connectionParameters: {
        client_id: 'guard-client-id',
        client_secret: 'stored-secret',
      },
    });

    await openConnectionEditor(page, title);

    // Editing a reauthorization-sensitive field and clicking Connect must not
    // silently authorize with the old stored value — it intercepts.
    await page.getByLabel('Client ID').fill('guard-client-id-changed');
    await page.getByRole('button', { name: 'Connect' }).click();

    const guard = page.getByRole('alertdialog');
    await expect(guard).toContainText('Save before reconnecting?');
    // A credential change resets authorization, so "Reconnect without saving"
    // is withheld — only Save & Reconnect or Cancel.
    await expect(
      guard.getByRole('button', { name: 'Reconnect without saving' })
    ).toHaveCount(0);
    await guard.getByRole('button', { name: 'Cancel' }).click();
    await expect(guard).toBeHidden();

    // Nothing was authorized or written: the stored client_id is unchanged.
    const saved = await getApiConnection(request, id);
    expect(saved.editProjection.values.client_id).toBe('guard-client-id');
    expect(saved.status).toBe('REQUIRES_RECONNECTION');
  });
});
