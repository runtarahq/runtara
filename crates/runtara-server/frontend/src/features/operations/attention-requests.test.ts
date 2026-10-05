import { beforeEach, describe, expect, it, vi } from 'vitest';
import type {
  OperationRequest,
  SavedOperationView,
} from '@/generated/RuntaraRuntimeApi';
import {
  attentionQueues,
  queryAttentionRequests,
  type AttentionQueue,
} from './attention-requests';
const request = vi.hoisted(() => vi.fn());
vi.mock('./queries', async () => ({
  ...(await vi.importActual('./queries')),
  operationsRequest: request,
}));
const now = Date.parse('2026-09-29T12:00:00Z');
function scope(id: string, count: number, due?: string): AttentionQueue {
  return {
    queue: {
      workflowId: id,
      workflowName: id,
      actionKey: 'review',
      name: 'Review',
      count,
      stateSchema: {},
    },
    view: {
      name: id,
      workflow: id,
      roles: due ? { due } : {},
      where: {
        openRequest: 'review',
        state: [{ field: 'unrelated', op: 'eq', value: { ignored: true } }],
      },
    },
  };
}
function rows(id: string, count: number): OperationRequest[] {
  return Array.from({ length: count }, (_, i) => ({
    actionKey: 'review',
    context: {},
    instanceId: id,
    workflowId: id,
    requestId: `${id}-${i}`,
    label: 'Review',
    requestedAt: '2026-09-29T10:00:00Z',
    usedVersion: 1,
  }));
}
beforeEach(() => {
  request.mockReset();
});
describe('request metric drilldowns', () => {
  it('pages every request across queue boundaries without preview limits or duplicate request identities', async () => {
    const scopes = [scope('C', 2), scope('A', 60), scope('B', 70)];
    const data = { A: rows('A', 60), B: rows('B', 70), C: rows('C', 2) };
    request.mockImplementation(
      async (_token, _path, _method, { workflowId, query }) => {
        const all = data[workflowId as keyof typeof data];
        return {
          content: all.slice(
            query.page * query.size,
            (query.page + 1) * query.size
          ),
          totalElements: all.length,
        };
      }
    );
    const ids: string[] = [];
    for (let page = 0; page < 6; page++) {
      const result = await queryAttentionRequests(
        '',
        scopes,
        false,
        page,
        25,
        now
      );
      expect(result.totalElements).toBe(132);
      expect(result.totalPages).toBe(6);
      expect(result.number).toBe(page);
      expect(result.content.length).toBe(page < 5 ? 25 : 7);
      ids.push(...result.content.map((item) => item.row.requestId));
    }
    expect(ids).toEqual(
      [...data.A, ...data.B, ...data.C].map((row) => row.requestId)
    );
    expect(new Set(ids).size).toBe(132);
    const last = await queryAttentionRequests('', scopes, false, 999, 25, now);
    expect(last.number).toBe(5);
    expect(last.content).toHaveLength(7);
  });
  it('uses each queue’s configured due field and the same cutoff, without unrelated saved-view filters', async () => {
    const scopes = [
      scope('A', 4, 'dueAt'),
      scope('B', 2, 'reviewBy'),
      scope('C', 9),
    ];
    request.mockResolvedValue({ content: [], totalElements: 0 });
    await queryAttentionRequests('', scopes, true, 0, 25, now);
    expect(request).toHaveBeenCalledTimes(2);
    for (const [, , , body] of request.mock.calls) {
      expect(body.query.state).toEqual([
        {
          field: body.workflowId === 'A' ? 'dueAt' : 'reviewBy',
          op: 'lt',
          value: new Date(now).toISOString(),
        },
      ]);
    }
    request.mockClear();
    await queryAttentionRequests('', scopes, false, 0, 25, now);
    expect(request).toHaveBeenCalledTimes(3);
    expect(
      request.mock.calls.every((call) => call[3].query.state.length === 0)
    ).toBe(true);
  });
  it('fails visibly if any queue cannot be loaded, instead of showing a partial total', async () => {
    request.mockRejectedValue(new Error('Unavailable'));
    await expect(
      queryAttentionRequests('', [scope('A', 1)], false, 0)
    ).rejects.toThrow('Unavailable');
  });
  it('selects the same due-bearing view for Overview and its request list', () => {
    const queue = scope('A', 1).queue;
    const views = [
      {
        configuration: {
          name: 'Filtered',
          workflow: 'A',
          where: { openRequest: 'review' },
          roles: {},
        },
      },
      {
        configuration: {
          name: 'Due',
          workflow: 'A',
          where: { openRequest: 'review' },
          roles: { due: 'dueAt' },
        },
      },
    ] as SavedOperationView[];
    expect(attentionQueues([queue], views)[0].view.roles?.due).toBe('dueAt');
  });
});
