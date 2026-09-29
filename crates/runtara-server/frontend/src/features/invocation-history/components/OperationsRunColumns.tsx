import { formatRunDuration } from '../utils/run-duration';
import type { ColumnDef } from '@tanstack/react-table';
import type { ExecutionHistoryItem } from '../types';
import { isActiveStatus } from '@/shared/utils/status-display';
import { ParentRunLink } from './RunLinks';
import {
  RunIdentity,
  RunContext,
  RunActions,
  RunDetailsToggle,
  RunTime,
} from './RunRow';

export type RunExtraColumn = 'completedAt' | 'parentInstanceId' | 'version';

export function operationsRunColumns(
  onReplay: (run: ExecutionHistoryItem) => void,
  expanded: ReadonlySet<string>,
  onDetailsChange: (id: string, open: boolean) => void,
  extraColumns: ReadonlySet<RunExtraColumn>
): ColumnDef<ExecutionHistoryItem>[] {
  return [
    {
      id: 'identity',
      header: 'Run',
      enableSorting: false,
      cell: ({ row }) => (
        <div className="flex w-52 items-center gap-2 xl:w-64">
          <RunDetailsToggle
            run={row.original}
            open={expanded.has(row.id)}
            onChange={onDetailsChange}
          />
          <RunIdentity run={row.original} />
        </div>
      ),
    },
    {
      id: 'context',
      header: 'Status / context',
      enableSorting: false,
      cell: ({ row }) => (
        <div className="w-48 xl:w-72">
          <RunContext run={row.original} />
        </div>
      ),
    },
    {
      accessorKey: 'createdAt',
      header: 'Started',
      enableSorting: true,
      cell: ({ row }) => <RunTime value={row.original.createdAt} />,
    },
    ...(extraColumns.has('completedAt')
      ? [
          {
            accessorKey: 'completedAt',
            header: 'Completed',
            enableSorting: true,
            cell: ({ row }) =>
              !isActiveStatus(row.original.status) &&
              row.original.completedAt ? (
                <RunTime value={row.original.completedAt} />
              ) : (
                '—'
              ),
          } satisfies ColumnDef<ExecutionHistoryItem>,
        ]
      : []),
    ...(extraColumns.has('parentInstanceId')
      ? [
          {
            accessorKey: 'parentInstanceId',
            header: 'Parent',
            enableSorting: false,
            cell: ({ row }) => (
              <ParentRunLink
                parentInstanceId={row.original.parentInstanceId}
                compact
              />
            ),
          } satisfies ColumnDef<ExecutionHistoryItem>,
        ]
      : []),
    ...(extraColumns.has('version')
      ? [
          {
            accessorKey: 'version',
            header: 'Version',
            enableSorting: false,
          } satisfies ColumnDef<ExecutionHistoryItem>,
        ]
      : []),
    {
      accessorKey: 'executionDurationSeconds',
      header: 'Duration',
      enableSorting: false,
      cell: ({ row }) => (
        <span className="whitespace-nowrap tabular-nums text-muted-foreground">
          {formatRunDuration(row.original.executionDurationSeconds)}
        </span>
      ),
    },
    {
      id: 'actions',
      header: 'Actions',
      enableSorting: false,
      cell: ({ row }) => (
        <div className="min-w-28">
          <RunActions run={row.original} onReplay={onReplay} />
        </div>
      ),
    },
  ];
}
