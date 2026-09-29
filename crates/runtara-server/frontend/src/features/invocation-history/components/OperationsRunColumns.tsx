import type { ColumnDef } from '@tanstack/react-table';
import type { ExecutionHistoryItem } from '../types';
import { invocationHistoryColumns } from './InvocationHistoryColumns';
import { RunIdentity, RunContext, RunActions } from './RunRow';

export function operationsRunColumns(
  onReplay: (run: ExecutionHistoryItem) => void,
  expandedErrors: ReadonlySet<string>,
  onDetailsChange: (id: string, open: boolean) => void
): ColumnDef<ExecutionHistoryItem>[] {
  const historical = invocationHistoryColumns.filter(
    (column) =>
      !['workflowId', 'status', 'actions'].includes(
        column.id ?? ('accessorKey' in column ? String(column.accessorKey) : '')
      )
  );
  return [
    {
      id: 'identity',
      header: 'Run',
      cell: ({ row }) => (
        <div className="w-44 xl:w-56">
          <RunIdentity run={row.original} />
        </div>
      ),
    },
    {
      id: 'context',
      header: 'Status and context',
      cell: ({ row }) => (
        <div className="w-48 xl:w-64">
          <RunContext
            run={row.original}
            detailsOpen={expandedErrors.has(row.original.instanceId)}
            onDetailsChange={onDetailsChange}
          />
        </div>
      ),
    },
    ...historical,
    {
      id: 'actions',
      header: 'Actions',
      cell: ({ row }) => (
        <div className="w-32">
          <RunActions run={row.original} onReplay={onReplay} />
        </div>
      ),
    },
  ];
}
