import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter, Route, Routes } from 'react-router';
import { CreateWorkflow } from './index';

const spies = vi.hoisted(() => ({
  createWorkflow: vi.fn(),
}));

// `main.tsx` mounts the app at import time; the page only needs its queryClient.
vi.mock('@/main.tsx', () => ({
  queryClient: { invalidateQueries: vi.fn() },
}));

vi.mock('react-oidc-context', () => ({
  useAuth: () => ({ user: { access_token: 'test-token' } }),
}));

vi.mock('@/features/workflows/queries', () => ({
  createWorkflow: spies.createWorkflow,
}));

vi.mock('@/features/workflows/hooks/useFolders', () => ({
  useFolders: () => ({
    data: {
      raw: ['/Demo/Test/'],
      parsed: [
        { path: '/Demo/', name: 'Demo', parentPath: '/', depth: 1 },
        { path: '/Demo/Test/', name: 'Test', parentPath: '/Demo/', depth: 2 },
      ],
      root: [{ path: '/Demo/', name: 'Demo', parentPath: '/', depth: 1 }],
      counts: [],
    },
    isError: false,
  }),
}));

function renderAt(url: string) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  return render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={[url]}>
        <Routes>
          <Route path="/workflows/create" element={<CreateWorkflow />} />
          <Route path="/workflows/:id" element={<div>editor</div>} />
        </Routes>
      </MemoryRouter>
    </QueryClientProvider>
  );
}

describe('CreateWorkflow', () => {
  beforeEach(() => {
    spies.createWorkflow.mockReset();
    spies.createWorkflow.mockResolvedValue({
      data: { id: 'wf-1' },
      message: 'Workflow has been created',
      success: true,
    });
  });

  it('creates the workflow in the folder from the URL', async () => {
    const user = userEvent.setup();
    renderAt('/workflows/create?folder=%2FDemo%2FTest%2F');

    await user.type(screen.getByLabelText('Name'), 'My workflow');
    await user.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() => expect(spies.createWorkflow).toHaveBeenCalledOnce());
    expect(spies.createWorkflow).toHaveBeenCalledWith(
      'test-token',
      expect.objectContaining({ name: 'My workflow', path: '/Demo/Test/' })
    );
  });

  it('defaults to the root without a folder param', async () => {
    const user = userEvent.setup();
    renderAt('/workflows/create');

    await user.type(screen.getByLabelText('Name'), 'Root workflow');
    await user.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() => expect(spies.createWorkflow).toHaveBeenCalledOnce());
    expect(spies.createWorkflow).toHaveBeenCalledWith(
      'test-token',
      expect.objectContaining({ name: 'Root workflow', path: '/' })
    );
  });

  it('ignores a malformed folder param instead of sending it', async () => {
    const user = userEvent.setup();
    renderAt('/workflows/create?folder=not-a-path');

    await user.type(screen.getByLabelText('Name'), 'Safe workflow');
    await user.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() => expect(spies.createWorkflow).toHaveBeenCalledOnce());
    expect(spies.createWorkflow).toHaveBeenCalledWith(
      'test-token',
      expect.objectContaining({ path: '/' })
    );
  });
});
