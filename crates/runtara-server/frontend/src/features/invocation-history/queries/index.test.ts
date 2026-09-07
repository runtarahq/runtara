import { expect, it, vi } from 'vitest';
import { getAllExecutions } from './index';

const list = vi.hoisted(() => vi.fn());
vi.mock('@/shared/queries', () => ({
  RuntimeREST: { api: { listAllExecutionsHandler: list } },
}));
vi.mock('@/shared/queries/utils', () => ({ createAuthHeaders: () => ({}) }));

it('forwards search, exact label, and pagination and retains labels and server totals', async () => {
  list.mockResolvedValue({
    data: {
      data: {
        content: [
          {
            id: 'run',
            workflowId: 'workflow',
            runLabel: 'Order/12',
            workflowName: 'Orders',
            created: '2026-09-07',
            status: 'completed',
            usedVersion: 1,
          },
        ],
        number: 2,
        size: 10,
        totalElements: 23,
        totalPages: 3,
      },
    },
  });
  const result = await getAllExecutions('', {
    queryKey: [
      'executions',
      {
        pageIndex: 2,
        pageSize: 10,
        filters: {
          search: 'order',
          runLabel: 'Order/12',
          workflowId: 'workflow',
        },
      },
    ],
  });
  expect(list.mock.calls[0][0]).toMatchObject({
    page: 2,
    size: 10,
    search: 'order',
    runLabel: 'Order/12',
    workflowId: 'workflow',
  });
  expect(result.content[0]).toMatchObject({
    runLabel: 'Order/12',
    workflowName: 'Orders',
  });
  expect(result).toMatchObject({ totalElements: 23, totalPages: 3, number: 2 });
});
