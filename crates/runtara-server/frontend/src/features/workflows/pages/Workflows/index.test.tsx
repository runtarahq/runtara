import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, useLocation, useNavigate } from 'react-router';
import { describe, expect, it, vi } from 'vitest';

import { Workflows } from './index';

vi.mock('react-oidc-context', () => ({
  useAuth: () => ({ user: { access_token: 'test-token' } }),
}));

vi.mock('../../hooks/useFolders', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../hooks/useFolders')>()),
  useFolders: () => ({ data: undefined, isError: false }),
  useFolderWorkflowCounts: () => ({}),
  useRenameFolder: () => ({ mutateAsync: vi.fn(), isPending: false }),
  useDeleteFolder: () => ({ mutateAsync: vi.fn(), isPending: false }),
}));

/** Stands in for the listing so the page's own URL handling is what is under test. */
vi.mock('../../components/WorkflowsGrid', () => ({
  WorkflowsGrid: ({
    toolbar,
    searchTerm,
  }: {
    toolbar: React.ReactNode;
    searchTerm: string;
  }) => (
    <>
      {toolbar}
      <output data-testid="grid-search-term">{searchTerm}</output>
    </>
  ),
}));

function LocationProbe() {
  const location = useLocation();
  const navigate = useNavigate();
  return (
    <>
      <output data-testid="location-search">{location.search}</output>
      <button onClick={() => navigate('/workflows?q=beta')}>go beta</button>
      <button onClick={() => navigate(-1)}>go back</button>
    </>
  );
}

function renderPage(initialEntry = '/workflows') {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });

  return render(
    <QueryClientProvider client={queryClient}>
      <MemoryRouter initialEntries={[initialEntry]}>
        <Workflows />
        <LocationProbe />
      </MemoryRouter>
    </QueryClientProvider>
  );
}

function searchBox(): HTMLInputElement {
  return screen.getByPlaceholderText('Search workflows…') as HTMLInputElement;
}

function locationSearch(): string {
  return screen.getByTestId('location-search').textContent ?? '';
}

describe('Workflows search URL state', () => {
  it('opens with the query the URL carries, in the box and in the listing', () => {
    renderPage('/workflows?q=alpha');

    expect(searchBox()).toHaveValue('alpha');
    expect(screen.getByTestId('grid-search-term')).toHaveTextContent('alpha');
  });

  it('writes what was typed once it settles, and forgets the page', async () => {
    renderPage('/workflows?page=3');

    await userEvent.click(screen.getByRole('button', { name: 'Search (⌘F)' }));
    await userEvent.type(searchBox(), 'demo');

    // Page 3 of the unfiltered listing means nothing to the filtered one.
    await waitFor(() => expect(locationSearch()).toBe('?q=demo'));
    expect(screen.getByTestId('grid-search-term')).toHaveTextContent('demo');
  });

  it('leaves the URL clean when the box is emptied again', async () => {
    renderPage('/workflows?q=alpha');

    await userEvent.clear(searchBox());

    await waitFor(() => expect(locationSearch()).toBe(''));
  });

  it('follows the URL when navigation changes it', async () => {
    renderPage('/workflows?q=alpha');

    await userEvent.click(screen.getByRole('button', { name: 'go beta' }));
    await waitFor(() => expect(searchBox()).toHaveValue('beta'));

    await userEvent.click(screen.getByRole('button', { name: 'go back' }));
    await waitFor(() => expect(searchBox()).toHaveValue('alpha'));
    expect(screen.getByTestId('grid-search-term')).toHaveTextContent('alpha');
  });
});
