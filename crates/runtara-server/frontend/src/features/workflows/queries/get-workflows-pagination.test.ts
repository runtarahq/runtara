import { beforeEach, describe, expect, it, vi } from 'vitest';

const listWorkflowsHandler = vi.fn();

vi.mock('@/shared/queries', () => ({
  RuntimeREST: { api: { listWorkflowsHandler } },
  createAuthHeaders: () => ({}),
}));

const { getWorkflows } = await import('./index');

/** The API caps pageSize at 100, so a page never holds more than that. */
function page(items: number, totalElements: number, totalPages: number) {
  return {
    data: {
      data: {
        content: Array.from({ length: items }, (_, i) => ({ id: `w${i}` })),
        number: 0,
        size: items,
        numberOfElements: items,
        totalElements,
        totalPages,
        first: true,
        last: totalPages <= 1,
      },
      message: 'Success',
      success: true,
    },
  };
}

/** Serve `total` workflows across 100-row pages, keyed by requested page. */
function serve(total: number) {
  const totalPages = Math.max(1, Math.ceil(total / 100));
  listWorkflowsHandler.mockImplementation((params: { page?: number }) => {
    const requested = params?.page ?? 1;
    const remaining = total - (requested - 1) * 100;
    return Promise.resolve(
      page(Math.max(0, Math.min(100, remaining)), total, totalPages)
    );
  });
  return totalPages;
}

describe('getWorkflows', () => {
  beforeEach(() => {
    listWorkflowsHandler.mockReset();
  });

  it('returns every workflow when the tenant spills past one page', async () => {
    serve(124);

    const result = (await getWorkflows('token')) as any;

    // The bug: this used to stop at 100 and the last 24 became unselectable.
    expect(result.data.content).toHaveLength(124);
    expect(listWorkflowsHandler).toHaveBeenCalledTimes(2);
  });

  it('asks for each page in turn', async () => {
    serve(124);

    await getWorkflows('token');

    const pages = listWorkflowsHandler.mock.calls.map((call) => call[0].page);
    expect(pages).toEqual([1, 2]);
  });

  it('makes a single request when everything fits on one page', async () => {
    serve(42);

    const result = (await getWorkflows('token')) as any;

    expect(result.data.content).toHaveLength(42);
    expect(listWorkflowsHandler).toHaveBeenCalledTimes(1);
  });

  it('restates the pagination fields to describe the merged list', async () => {
    serve(124);

    const result = (await getWorkflows('token')) as any;

    expect(result.data).toMatchObject({
      number: 0,
      size: 124,
      numberOfElements: 124,
      totalPages: 1,
      first: true,
      last: true,
      totalElements: 124, // the true total is preserved
    });
  });

  it('stops at the page ceiling rather than issuing unbounded requests', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    serve(100 * 40); // 40 pages, double the ceiling

    const result = (await getWorkflows('token')) as any;

    expect(listWorkflowsHandler).toHaveBeenCalledTimes(20);
    expect(result.data.content).toHaveLength(2000);
    // `last: false` marks the list as knowingly incomplete.
    expect(result.data.last).toBe(false);
    expect(warn).toHaveBeenCalled();

    warn.mockRestore();
  });
});
