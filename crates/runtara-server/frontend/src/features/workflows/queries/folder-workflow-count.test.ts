import { beforeEach, describe, expect, it, vi } from 'vitest';

const listWorkflowsHandler = vi.fn();

vi.mock('@/shared/queries', () => ({
  RuntimeREST: { api: { listWorkflowsHandler } },
  createAuthHeaders: () => ({}),
}));

const { getFolderWorkflowCount } = await import('./index');

function respondWith(totalElements: unknown, content: unknown[] = []) {
  listWorkflowsHandler.mockResolvedValue({
    data: { data: { content, totalElements } },
  });
}

describe('getFolderWorkflowCount', () => {
  beforeEach(() => {
    listWorkflowsHandler.mockReset();
  });

  it('counts recursively so workflows in subfolders are included', async () => {
    respondWith(6);

    await getFolderWorkflowCount('token', '/Demo/');

    expect(listWorkflowsHandler).toHaveBeenCalledTimes(1);
    expect(listWorkflowsHandler.mock.calls[0][0]).toMatchObject({
      path: '/Demo/',
      recursive: true,
    });
  });

  it('reads the server total rather than counting returned rows', async () => {
    // The whole point: one row comes back, but the folder holds 124.
    respondWith(124, [{ id: 'only-one' }]);

    await expect(getFolderWorkflowCount('token', '/Commerce/')).resolves.toBe(
      124
    );
  });

  it('asks for a single row so the count costs no payload', async () => {
    respondWith(124);

    await getFolderWorkflowCount('token', '/Commerce/');

    expect(listWorkflowsHandler.mock.calls[0][0]).toMatchObject({
      pageSize: 1,
    });
  });

  it('falls back to 0 when the response carries no total', async () => {
    listWorkflowsHandler.mockResolvedValue({ data: { data: {} } });

    await expect(getFolderWorkflowCount('token', '/Empty/')).resolves.toBe(0);
  });
});
