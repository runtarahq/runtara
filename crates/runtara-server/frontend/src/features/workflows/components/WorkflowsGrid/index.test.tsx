import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter } from 'react-router';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import type { WorkflowDto } from '@/generated/RuntaraRuntimeApi';
import { WorkflowsGrid } from './index';

const FOLDER_COUNT = 12;
const WORKFLOW_COUNT = 43;
const PAGE_SIZE = 10;

const mocks = vi.hoisted(() => ({
  getWorkflowsInFolder: vi.fn(),
}));

vi.mock('react-oidc-context', () => ({
  useAuth: () => ({ user: { access_token: 'test-token' } }),
}));

vi.mock('@/main.tsx', () => ({
  queryClient: { invalidateQueries: vi.fn() },
}));

vi.mock('../../hooks/useFolders', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../hooks/useFolders')>()),
  useFolders: () => ({ data: undefined }),
}));

vi.mock('@/features/workflows/queries', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@/features/workflows/queries')>()),
  getWorkflowsInFolder: mocks.getWorkflowsInFolder,
}));

/** Workflow N is the Nth most recent, so the default "updated desc" sort keeps 1…43 order. */
function buildWorkflow(index: number): WorkflowDto {
  const updated = new Date(
    Date.UTC(2026, 0, 1) - index * 3_600_000
  ).toISOString();

  return {
    id: `wf-${index}`,
    name: `Workflow ${String(index).padStart(2, '0')}`,
    description: '',
    created: updated,
    updated,
    currentVersionNumber: 1,
    lastVersionNumber: 1,
    executionGraph: {},
    inputSchema: {},
    outputSchema: {},
    path: '/',
  } as unknown as WorkflowDto;
}

const allWorkflows = Array.from({ length: WORKFLOW_COUNT }, (_, i) =>
  buildWorkflow(i + 1)
);

const allFolders = Array.from({ length: FOLDER_COUNT }, (_, i) => {
  const name = `folder-${String(i + 1).padStart(2, '0')}`;
  return { name, path: `/${name}/` };
});

/** Stands in for GET /api/runtime/workflows, honouring its page/pageSize contract. */
function serveWorkflowPage(_token: string, params: Record<string, any>) {
  const pageSize = params.pageSize ?? 20;
  const page = params.page ?? 0;
  const start = page * pageSize;
  const content = allWorkflows.slice(start, start + pageSize);

  return Promise.resolve({
    data: {
      content,
      first: page === 0,
      last: start + pageSize >= WORKFLOW_COUNT,
      number: page,
      numberOfElements: content.length,
      size: pageSize,
      totalElements: WORKFLOW_COUNT,
      totalPages: Math.ceil(WORKFLOW_COUNT / pageSize),
    },
  });
}

function renderGrid() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });

  return render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={['/workflows']}>
        <WorkflowsGrid searchTerm="" folderPath="/" folders={allFolders} />
      </MemoryRouter>
    </QueryClientProvider>
  );
}

function folderRowNames(): string[] {
  return allFolders
    .map((folder) => folder.name)
    .filter((name) => screen.queryByText(name) !== null);
}

async function goToNextPage() {
  await userEvent.click(screen.getByRole('button', { name: 'Next page' }));
}

describe('WorkflowsGrid pagination', () => {
  beforeEach(() => {
    mocks.getWorkflowsInFolder.mockReset();
    mocks.getWorkflowsInFolder.mockImplementation(serveWorkflowPage);
  });

  it('pages folders and workflows as one row set instead of repeating folders', async () => {
    renderGrid();

    // Page 1 — 12 folders overflow a 10-row page, so it is folders only.
    await screen.findByText('folder-01');
    expect(folderRowNames()).toEqual([
      'folder-01',
      'folder-02',
      'folder-03',
      'folder-04',
      'folder-05',
      'folder-06',
      'folder-07',
      'folder-08',
      'folder-09',
      'folder-10',
    ]);
    expect(screen.queryByText('Workflow 01')).not.toBeInTheDocument();
    expect(
      screen.getByText(`Rows 1–10 of 55 · ${FOLDER_COUNT} folders`)
    ).toBeInTheDocument();

    // Page 2 — the last two folders, then workflows from the very beginning.
    await goToNextPage();
    await screen.findByText('Workflow 01');
    expect(folderRowNames()).toEqual(['folder-11', 'folder-12']);
    expect(screen.getByText('Workflow 08')).toBeInTheDocument();
    expect(screen.queryByText('Workflow 09')).not.toBeInTheDocument();
    expect(
      screen.getByText('Rows 11–20 of 55 · 12 folders')
    ).toBeInTheDocument();

    // Page 3 — no folder rows at all: the bug rendered all 12 again right here.
    await goToNextPage();
    await screen.findByText('Workflow 09');
    expect(folderRowNames()).toEqual([]);
    expect(screen.getByText('Workflow 18')).toBeInTheDocument();
    expect(screen.queryByText('Workflow 08')).not.toBeInTheDocument();
    expect(screen.queryByText('Workflow 19')).not.toBeInTheDocument();
    expect(
      screen.getByText('Rows 21–30 of 55 · 12 folders')
    ).toBeInTheDocument();
  });

  it('stitches the two server pages a folder-shifted window straddles', async () => {
    renderGrid();

    await screen.findByText('folder-01');
    await goToNextPage();
    await goToNextPage();
    await screen.findByText('Workflow 09');

    // Workflows 9–18 live across server pages 0 and 1 of a 10-row page size.
    const requestedPages = mocks.getWorkflowsInFolder.mock.calls
      .filter(([, params]) => params.pageSize === PAGE_SIZE)
      .map(([, params]) => params.page);
    expect(requestedPages).toContain(0);
    expect(requestedPages).toContain(1);
  });

  it('fills the first page with workflows when the folder list is short', async () => {
    render(
      <QueryClientProvider
        client={
          new QueryClient({ defaultOptions: { queries: { retry: false } } })
        }
      >
        <MemoryRouter initialEntries={['/workflows']}>
          <WorkflowsGrid
            searchTerm=""
            folderPath="/"
            folders={allFolders.slice(0, 3)}
          />
        </MemoryRouter>
      </QueryClientProvider>
    );

    await screen.findByText('Workflow 01');
    expect(folderRowNames()).toEqual(['folder-01', 'folder-02', 'folder-03']);
    expect(screen.getByText('Workflow 07')).toBeInTheDocument();
    expect(screen.queryByText('Workflow 08')).not.toBeInTheDocument();
    expect(screen.getByText('Rows 1–10 of 46 · 3 folders')).toBeInTheDocument();

    await goToNextPage();
    await screen.findByText('Workflow 08');
    expect(folderRowNames()).toEqual([]);
    expect(screen.getByText('Workflow 17')).toBeInTheDocument();
  });
});
