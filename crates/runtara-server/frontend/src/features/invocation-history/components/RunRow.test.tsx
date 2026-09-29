import { render, screen, fireEvent, act } from '@testing-library/react';
import { MemoryRouter } from 'react-router';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { useAuthStore } from '@/shared/stores/authStore';
import type { ExecutionHistoryItem } from '../types';
import { RunActions, RunContext, RunIdentity, RunDetails } from './RunRow';
vi.mock('@/features/workflows/components/ResumeButton', () => ({
  ResumeButton: () => <button>Resume</button>,
}));
vi.mock('@/features/workflows/components/StopButton', () => ({
  StopButton: () => <button>Stop</button>,
}));
const run: ExecutionHistoryItem = {
  instanceId: 'run-12345678',
  workflowId: 'workflow',
  workflowName: 'Orders',
  runLabel: 'ORDER-123',
  status: 'failed',
  version: 1,
  createdAt: '2026-09-29T10:00:00Z',
};
afterEach(() => act(() => useAuthStore.getState().clearMe()));
describe('operational run rows', () => {
  it('links the business label to Operations, keeps execution details, and opens Replay with the original row', () => {
    const replay = vi.fn();
    render(
      <MemoryRouter>
        <RunIdentity run={run} />
        <RunActions run={run} onReplay={replay} />
      </MemoryRouter>
    );
    expect(screen.getByRole('link', { name: 'ORDER-123' })).toHaveAttribute(
      'href',
      '/operations/runs/workflow/run-12345678'
    );
    expect(
      screen.getByRole('link', { name: 'Open execution' })
    ).toHaveAttribute('href', '/workflows/workflow/history/run-12345678');
    fireEvent.click(screen.getByRole('button', { name: 'Replay' }));
    expect(replay).toHaveBeenCalledWith(run);
  });
  it.each([
    'waiting_signal',
    'waiting_instances',
    'sleeping',
    'shutdown',
    'paused',
  ] as const)('reserves Resume for paused runs: %s', (reason) => {
    const waiting = {
      ...run,
      status: 'suspended' as const,
      suspensionReason: reason,
    };
    render(
      <MemoryRouter>
        <RunContext run={waiting} />
        <RunActions run={waiting} onReplay={vi.fn()} />
      </MemoryRouter>
    );
    expect(screen.queryByRole('button', { name: 'Resume' }) !== null).toBe(
      reason === 'paused'
    );
    expect(
      screen.getByRole('link', { name: 'Review run and requests' })
    ).toBeInTheDocument();
    expect(screen.queryByText(/Started/)).not.toBeInTheDocument();
    expect(screen.queryByText(/^Completed /)).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Stop' })).toBeInTheDocument();
  });
  it('keeps reads and hides every mutation for a read-only user', () => {
    useAuthStore.getState().setMe({
      role: 'viewer',
      permissions: { 'invocation_history:read': true } as never,
    });
    render(
      <MemoryRouter>
        <RunActions run={run} onReplay={vi.fn()} />
        <RunActions
          run={{ ...run, status: 'suspended', suspensionReason: 'paused' }}
          onReplay={vi.fn()}
        />
      </MemoryRouter>
    );
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
    expect(
      screen.getAllByRole('link', { name: 'Open execution' })
    ).toHaveLength(2);
  });
  it.each([
    ['host failure', 'host failure'],
    [null, 'No error details recorded'],
    ['{"message":"Try later","code":"TEMP"}', 'Try later'],
  ])('renders error fallback %s', (error, message) => {
    render(
      <MemoryRouter>
        <RunContext run={{ ...run, error }} />
      </MemoryRouter>
    );
    expect(screen.getByText(message!)).toBeInTheDocument();
  });
});

it('keeps full errors and completion metadata in expanded details', () => {
  const message = 'A long error that must remain readable in full';
  render(
    <MemoryRouter>
      <RunDetails
        run={{
          ...run,
          error: JSON.stringify({
            message,
            code: 'TEMP',
            category: 'transient',
          }),
          completedAt: '2026-09-29T10:01:14Z',
        }}
      />
    </MemoryRouter>
  );
  expect(screen.getByText(message)).toBeInTheDocument();
  expect(screen.getByText('TEMP · transient')).toBeInTheDocument();
  expect(screen.getByText('Completed')).toBeInTheDocument();
  expect(screen.getByText(run.instanceId)).toBeInTheDocument();
});
