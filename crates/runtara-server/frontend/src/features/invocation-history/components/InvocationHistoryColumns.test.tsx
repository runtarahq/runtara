import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router';
import type { ExecutionHistoryItem } from '../types';
import { invocationHistoryColumns } from './InvocationHistoryColumns';

vi.mock('@/features/workflows/components/ResumeButton', () => ({
  ResumeButton: () => <button>Resume</button>,
}));
vi.mock('@/features/workflows/components/StopButton', () => ({
  StopButton: () => <button>Stop</button>,
}));
vi.mock('@/features/workflows/components/ReplayButton', () => ({
  ReplayButton: () => <button>Replay</button>,
}));
vi.mock('./RunLinks', () => ({
  ParentRunLink: ({ parentInstanceId }: { parentInstanceId?: string }) => (
    <span>parent:{parentInstanceId ?? '—'}</span>
  ),
  RunStatusPill: ({ status }: { status: string }) => <span>{status}</span>,
}));

function renderCell(columnId: string, row: Partial<ExecutionHistoryItem>) {
  const column = invocationHistoryColumns.find(
    (c) =>
      c.id === columnId ||
      (c as { accessorKey?: string }).accessorKey === columnId
  )!;
  const original = {
    instanceId: 'run-1',
    workflowId: 'wf-1',
    createdAt: '2026-09-27',
    version: 1,
    status: 'completed',
    ...row,
  } as ExecutionHistoryItem;
  const cell = column.cell as (ctx: unknown) => React.ReactNode;
  return render(
    <MemoryRouter>
      {cell({
        row: {
          original,
          getValue: (key: string) =>
            original[key as keyof ExecutionHistoryItem],
        },
      })}
    </MemoryRouter>
  );
}

describe('invocation history columns', () => {
  it('never offers Resume on failed or cancelled runs', () => {
    for (const status of ['failed', 'cancelled'] as const) {
      const { unmount } = renderCell('actions', { status });
      expect(screen.queryByText('Resume')).not.toBeInTheDocument();
      expect(screen.getByText('Replay')).toBeInTheDocument();
      unmount();
    }
  });

  it('offers Resume only for a paused suspended run', () => {
    const paused = renderCell('actions', {
      status: 'suspended',
      suspensionReason: 'paused',
    });
    expect(screen.getByText('Resume')).toBeInTheDocument();
    expect(
      screen.getByLabelText('Open in editor — resume debugging')
    ).toBeInTheDocument();
    paused.unmount();

    renderCell('actions', {
      status: 'suspended',
      suspensionReason: 'waiting_instances',
    });
    expect(screen.queryByText('Resume')).not.toBeInTheDocument();
    expect(screen.getByText('Stop')).toBeInTheDocument();
    expect(screen.getByLabelText('Open in editor')).toBeInTheDocument();
  });

  it('renders the Parent column from parentInstanceId', () => {
    renderCell('parentInstanceId', { parentInstanceId: 'parent-1' });
    expect(screen.getByText('parent:parent-1')).toBeInTheDocument();
  });
});
