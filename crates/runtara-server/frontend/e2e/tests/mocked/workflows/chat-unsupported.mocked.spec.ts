import type { Route } from '@playwright/test';
import { buildWorkflow, expect, test } from '../../../fixtures';
import { WorkflowChatPage } from '../../../pages/WorkflowExtraPages';
import { appPath } from '../../../utils/app-path';

/** Same shape the mock fixture uses, so tenant-prefixed URLs still match. */
function runtimeUrl(suffix: string): RegExp {
  const escaped = suffix.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return new RegExp(`/api/runtime(?:/[^/]+)?/${escaped}(?:\\?[^/]*)?$`);
}

test.describe('Workflow chat on an unsupported workflow (mocked)', () => {
  test('withholds the Chat action from a workflow that never waits', async ({
    page,
    mockApi,
  }) => {
    const chattable = buildWorkflow({
      id: 'scn_chat_yes',
      name: 'Support agent',
      supportsChat: true,
    });
    const plain = buildWorkflow({
      id: 'scn_chat_no',
      name: 'Nightly export',
      supportsChat: false,
    });

    await mockApi.bootstrap(page);
    await mockApi.workflows.list(page, [chattable, plain]);

    await page.goto(appPath('/workflows'));

    // Both rows render, so the single missing action below is the gate rather
    // than a list that failed to load.
    const chattableRow = page.getByRole('row', { name: /Support agent/ });
    const plainRow = page.getByRole('row', { name: /Nightly export/ });
    await expect(chattableRow).toBeVisible();
    await expect(plainRow).toBeVisible();

    await expect(chattableRow.getByTitle('Chat')).toHaveCount(1);
    await expect(plainRow.getByTitle('Chat')).toHaveCount(0);
    await expect(plainRow.getByTitle('Start')).toHaveCount(1);
  });

  test('explains the dead end and opens no session on a direct URL', async ({
    page,
    mockApi,
  }) => {
    const workflow = buildWorkflow({
      id: 'scn_chat_none',
      name: 'Nightly export',
      supportsChat: false,
    });

    let sessionRequests = 0;

    await mockApi.bootstrap(page);
    await mockApi.workflows.get(page, workflow.id, workflow);
    await mockApi.raw(
      page,
      runtimeUrl(`workflows/${workflow.id}/sessions`),
      async (route: Route) => {
        sessionRequests += 1;
        await route.fulfill({ status: 200, body: '' });
      }
    );

    // Bookmarks and hand-typed URLs still reach the page — hiding the row
    // action removes the dead end, not the safety net.
    const view = new WorkflowChatPage(page, workflow.id);
    await view.goto();

    await expect(
      page.getByText('This workflow does not support chat')
    ).toBeVisible();
    await expect(page.getByPlaceholder('Type a message...')).toHaveCount(0);
    expect(sessionRequests).toBe(0);
  });
});
