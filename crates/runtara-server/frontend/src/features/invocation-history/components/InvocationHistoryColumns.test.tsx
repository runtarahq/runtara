import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MemoryRouter } from 'react-router';
import { DataTable } from '@/shared/components/table';
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
  ParentRunLink: ({
    parentInstanceId,
    compact,
  }: {
    parentInstanceId?: string;
    compact?: boolean;
  }) => (
    <span>
      parent:{parentInstanceId ?? '—'}
      {compact ? ':compact' : ''}
    </span>
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

  it('renders the Parent column compactly from parentInstanceId', () => {
    renderCell('parentInstanceId', { parentInstanceId: 'parent-1' });
    expect(screen.getByText('parent:parent-1:compact')).toBeInTheDocument();
  });

  it('shows dates as date over time with the full timestamp on hover', () => {
    renderCell('createdAt', { createdAt: '2026-09-27T20:56:10Z' });
    const cell = screen.getByTitle(/27 Sep, 2026/);
    expect(cell.children).toHaveLength(2);
    expect(cell.children[0]).toHaveTextContent(/^27 Sep, 2026$/);
    expect(cell.children[1]).toHaveTextContent(/\d{1,2}:\d{2}/);
  });

  it('hides lower-priority columns on narrow viewports', () => {
    render(
      <MemoryRouter>
        <DataTable
          columns={invocationHistoryColumns}
          data={[
            {
              instanceId: 'run-1',
              workflowId: 'wf-1',
              createdAt: '2026-09-27T20:56:10Z',
              completedAt: '2026-09-27T20:57:10Z',
              status: 'completed',
              version: 1,
              parentInstanceId: 'parent-1',
              executionDurationSeconds: 60,
            } as ExecutionHistoryItem,
          ]}
          stickyHeader
          shouldRenderPagination={false}
        />
      </MemoryRouter>
    );
    const headerClass = (name: string) =>
      screen.getByRole('columnheader', { name: new RegExp(name, 'i') })
        .className;
    expect(headerClass('Parent')).toContain('hidden min-[1440px]:table-cell');
    expect(headerClass('Completed')).toContain(
      'hidden min-[1600px]:table-cell'
    );
    expect(headerClass('Duration')).toContain('hidden xl:table-cell');
    expect(headerClass('Version')).toContain('hidden min-[1800px]:table-cell');
    for (const name of ['Execution', 'Started', 'Status', 'Actions'])
      expect(headerClass(name)).not.toContain('hidden');
    // Cells hide with their header.
    const cells = screen.getAllByRole('cell');
    const parentCell = screen
      .getByText('parent:parent-1:compact')
      .closest('td');
    expect(parentCell).toHaveClass('hidden', 'min-[1440px]:table-cell');
    expect(cells.filter((c) => c.className.includes('hidden'))).toHaveLength(4);
  });
});
