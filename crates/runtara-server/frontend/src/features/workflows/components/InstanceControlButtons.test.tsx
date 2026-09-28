import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const auth = vi.hoisted(() => ({ oidc: false, token: '' }));
const queries = vi.hoisted(() => ({
  stopInstance: vi.fn().mockResolvedValue({}),
  pauseInstance: vi.fn().mockResolvedValue({}),
  resumeInstance: vi.fn().mockResolvedValue({}),
}));
const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }));

vi.mock('@/shared/config/runtimeConfig', () => ({
  get isOidcAuth() {
    return auth.oidc;
  },
}));
vi.mock('@/shared/hooks', () => ({ useToken: () => auth.token }));
vi.mock('@/features/workflows/queries', () => queries);
vi.mock('sonner', () => ({ toast }));
vi.mock('@/shared/components/ui/tooltip.tsx', () => ({
  WithTooltip: ({ children }: { children: React.ReactNode }) => children,
}));

import { StopButton } from './StopButton';
import { PauseButton } from './PauseButton';
import { ResumeButton } from './ResumeButton';
import { queryKeys } from '@/shared/queries/query-keys';

let queryClient: QueryClient;

function renderWithQueries(ui: React.ReactElement) {
  return render(
    <QueryClientProvider client={queryClient}>{ui}</QueryClientProvider>
  );
}

const cases = [
  {
    name: 'Stop',
    call: queries.stopInstance,
    click: () => {
      renderWithQueries(<StopButton instanceId="run-1" />);
      fireEvent.click(screen.getByRole('button', { name: 'Stop' }));
    },
  },
  {
    name: 'Pause',
    call: queries.pauseInstance,
    click: () => {
      renderWithQueries(<PauseButton instanceId="run-1" />);
      fireEvent.click(screen.getByRole('button', { name: 'Pause' }));
      fireEvent.click(screen.getByRole('button', { name: 'Pause Instance' }));
    },
  },
  {
    name: 'Resume',
    call: queries.resumeInstance,
    click: () => {
      renderWithQueries(<ResumeButton instanceId="run-1" />);
      fireEvent.click(
        screen.getByRole('button', { name: 'Resume from last checkpoint' })
      );
    },
  },
];

describe.each(cases)('$name button', ({ call, click }) => {
  beforeEach(() => {
    vi.clearAllMocks();
    queryClient = new QueryClient();
  });

  it('sends the request without a token when auth is not OIDC', async () => {
    auth.oidc = false;
    auth.token = '';
    click();
    await waitFor(() => expect(call).toHaveBeenCalledWith('', 'run-1'));
    expect(toast.error).not.toHaveBeenCalled();
  });

  it('refreshes the run lists after the request succeeds', async () => {
    auth.oidc = false;
    auth.token = '';
    const invalidate = vi.spyOn(queryClient, 'invalidateQueries');
    click();
    await waitFor(() =>
      expect(invalidate).toHaveBeenCalledWith({
        queryKey: queryKeys.executions.lists(),
      })
    );
  });

  it('sends the request with the OIDC token', async () => {
    auth.oidc = true;
    auth.token = 'access-token';
    click();
    await waitFor(() =>
      expect(call).toHaveBeenCalledWith('access-token', 'run-1')
    );
  });

  it('reports an expired OIDC session instead of doing nothing', async () => {
    auth.oidc = true;
    auth.token = '';
    click();
    await waitFor(() => expect(toast.error).toHaveBeenCalledTimes(1));
    expect(call).not.toHaveBeenCalled();
  });
});
