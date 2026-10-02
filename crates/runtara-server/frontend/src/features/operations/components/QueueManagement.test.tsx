import {
  act,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { useAuthStore } from '@/shared/stores/authStore';
import type { SavedOperationView } from '@/generated/RuntaraRuntimeApi';
import { DeleteQueue } from './DeleteQueue';
import { ViewEditor } from './ViewEditor';
const api = vi.hoisted(() => vi.fn());
vi.mock('@/shared/hooks', () => ({ useToken: () => 'test-token' }));
vi.mock('../queries', async () => ({
  ...(await vi.importActual('../queries')),
  operationsRequest: api,
}));
const saved: SavedOperationView = {
  id: 'queue',
  revision: 1,
  updatedAt: '2026-09-29T12:00:00Z',
  configuration: {
    name: 'Orders',
    workflow: 'workflow',
    where: {},
    columns: [],
  },
};
const wrapper = ({ children }: { children: React.ReactNode }) => (
  <QueryClientProvider
    client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}
  >
    {children}
  </QueryClientProvider>
);
beforeEach(() => {
  api.mockReset();
});
afterEach(() => {
  act(() => useAuthStore.getState().clearMe());
});

describe('queue management', () => {
  it('requires confirmation and deletes only the revision presented when opened', async () => {
    api.mockResolvedValue(undefined);
    const onDeleted = vi.fn();
    const { rerender } = render(
      <DeleteQueue queue={saved} onDeleted={onDeleted} />,
      { wrapper }
    );
    fireEvent.click(
      screen.getByRole('button', { name: 'Delete queue Orders' })
    );
    expect(api).not.toHaveBeenCalled();
    rerender(
      <DeleteQueue queue={{ ...saved, revision: 2 }} onDeleted={onDeleted} />
    );
    fireEvent.click(screen.getByRole('button', { name: 'Delete queue' }));
    await waitFor(() => expect(onDeleted).toHaveBeenCalledOnce());
    expect(api).toHaveBeenCalledWith(
      'test-token',
      'operations/views/queue?revision=1',
      'DELETE'
    );
  });
  it('retains the dialog and surfaces a conflict without treating deletion as successful', async () => {
    api.mockRejectedValue(
      new Error('This queue changed. Reload before deleting.')
    );
    const onDeleted = vi.fn();
    render(<DeleteQueue queue={saved} onDeleted={onDeleted} />, { wrapper });
    fireEvent.click(
      screen.getByRole('button', { name: 'Delete queue Orders' })
    );
    fireEvent.click(screen.getByRole('button', { name: 'Delete queue' }));
    await waitFor(() =>
      expect(screen.getByRole('alert')).toHaveTextContent(
        'Reload before deleting'
      )
    );
    expect(screen.getByRole('dialog')).toBeInTheDocument();
    expect(onDeleted).not.toHaveBeenCalled();
  });
  it('saves against the editor’s original revision after metadata refresh', async () => {
    api.mockResolvedValue(saved);
    const onSaved = vi.fn();
    const { rerender } = render(
      <ViewEditor
        initial={saved.configuration}
        saved={saved}
        schema={{}}
        onSaved={onSaved}
        onCancel={vi.fn()}
      />,
      { wrapper }
    );
    rerender(
      <ViewEditor
        initial={saved.configuration}
        saved={{ ...saved, revision: 2 }}
        schema={{}}
        onSaved={onSaved}
        onCancel={vi.fn()}
      />
    );
    fireEvent.change(screen.getByRole('textbox', { name: 'Name' }), {
      target: { value: 'Changed' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    await waitFor(() => expect(onSaved).toHaveBeenCalled());
    expect(api.mock.calls[0][3]).toMatchObject({
      revision: 1,
      configuration: { name: 'Changed' },
    });
  });
  it('does not expose delete to a read-only member', () => {
    useAuthStore.getState().setMe({
      role: 'viewer',
      permissions: { 'invocation_history:read': true } as never,
    });
    render(<DeleteQueue queue={saved} />, { wrapper });
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
  });
});
