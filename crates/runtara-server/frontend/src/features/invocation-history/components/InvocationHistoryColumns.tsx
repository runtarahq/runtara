import { executionDisplayName } from '@/features/workflows/utils/run-label';
import { ColumnDef } from '@tanstack/react-table';
import { Link } from 'react-router';
import { ExternalLink, Eye, Zap, MessageSquare, Bug } from 'lucide-react';
import { ExecutionHistoryItem } from '../types';
import { cn, formatDate } from '@/lib/utils';
import { Button } from '@/shared/components/ui/button';
import { WithTooltip } from '@/shared/components/ui/tooltip';
import { statusToneClasses } from '@/shared/components/console';
import { isActiveStatus } from '@/shared/utils/status-display';
import { ReplayButton } from '@/features/workflows/components/ReplayButton';
import { ResumeButton } from '@/features/workflows/components/ResumeButton';
import { StopButton } from '@/features/workflows/components/StopButton';
import { canResume } from '@/features/workflows/utils/suspension';
import { ParentRunLink, RunStatusPill } from './RunLinks';
import {
  responsiveColumnClass,
  type InvocationColumnId,
} from '../utils/column-layout';

/** Column meta that hides a lower-priority column on narrower viewports. */
const responsiveMeta = (columnId: InvocationColumnId) => {
  const className = responsiveColumnClass(columnId);
  return { headerClassName: className, cellClassName: className };
};

/**
 * Date over time on two lines, so the Started/Completed columns stay narrow.
 * The full timestamp is the hover text.
 */
const dateTimeCell = (value: string) => {
  return (
    <div className="flex flex-col" title={formatDate(value)}>
      <span className="text-sm text-foreground">
        {formatDate(value, 'dd MMM, yyyy')}
      </span>
      <span className="text-xs text-muted-foreground">
        {formatDate(value, 'p')}
      </span>
    </div>
  );
};

// Helper to format duration. A negative value is meaningless (it comes from a
// stale suspend `finished_at` predating a resumed run's `started_at`); render
// it as blank rather than a bogus "-15s".
const formatDuration = (seconds: number | null | undefined): string => {
  if (seconds === null || seconds === undefined || seconds < 0) return '-';
  const ms = seconds * 1000;
  if (ms < 1000) return `${Math.round(ms)}ms`;
  if (seconds < 60) return `${seconds.toFixed(1)}s`;
  return `${Math.floor(seconds / 60)}m ${Math.round(seconds % 60)}s`;
};

// Helper to get duration color based on time. Negatives are neutral, never the
// success "fast run" branch.
const getDurationColorClass = (seconds: number | null | undefined): string => {
  if (seconds === null || seconds === undefined || seconds < 0)
    return 'text-muted-foreground';
  const ms = seconds * 1000;
  if (ms < 100) return 'text-success';
  if (ms < 1000) return 'text-muted-foreground';
  if (ms < 5000) return 'text-warning';
  return 'text-destructive';
};

export const invocationHistoryColumns: ColumnDef<ExecutionHistoryItem>[] = [
  {
    id: 'workflowId',
    accessorKey: 'workflowName',
    header: 'Execution',
    enableSorting: false,
    cell: ({ row }) => {
      const workflowId = row.original.workflowId;
      const workflowName = executionDisplayName(row.original);
      const instanceId = row.original.instanceId;

      return (
        <div className="flex flex-col gap-0.5">
          {workflowId ? (
            <Link
              to={`/workflows/${workflowId}`}
              className="group/link inline-flex items-center gap-1.5 text-sm font-medium text-foreground hover:text-primary"
            >
              <span className="max-w-60 truncate" title={workflowName}>
                {workflowName}
              </span>
              <ExternalLink className="size-3 text-muted-foreground transition-colors group-hover/link:text-primary" />
            </Link>
          ) : (
            <span className="text-sm font-medium italic text-muted-foreground">
              {workflowName}
            </span>
          )}
          {row.original.runLabel && row.original.workflowName && (
            <span
              className="max-w-60 truncate text-xs text-muted-foreground"
              title={row.original.workflowName}
            >
              {row.original.workflowName}
            </span>
          )}
          <span className="font-mono text-xs text-muted-foreground">
            {instanceId}
          </span>
        </div>
      );
    },
  },
  {
    accessorKey: 'createdAt',
    header: 'Started',
    enableSorting: true,
    cell: ({ row }) => {
      const createdAt: string = row.getValue('createdAt');
      return dateTimeCell(createdAt);
    },
  },
  {
    accessorKey: 'completedAt',
    header: 'Completed',
    enableSorting: true,
    meta: responsiveMeta('completedAt'),
    cell: ({ row }) => {
      const completedAt = row.original.completedAt;
      // A non-terminal row (running/suspended/…) has no real completion time;
      // its `completedAt` is a suspend/drain timestamp, so don't present it as
      // "Completed".
      if (!completedAt || isActiveStatus(row.original.status)) {
        return <span className="text-sm text-muted-foreground">-</span>;
      }
      return dateTimeCell(completedAt);
    },
  },
  {
    accessorKey: 'status',
    header: 'Status',
    enableSorting: false,
    cell: ({ row }) => {
      const status: string = row.getValue('status');
      const hasPendingInput = row.original.hasPendingInput;
      return (
        <div className="flex items-center gap-1.5">
          <RunStatusPill
            status={status}
            suspensionReason={row.original.suspensionReason}
            className="min-w-[90px]"
          />
          {hasPendingInput && (
            <WithTooltip label="Continue chat">
              <Link
                to={`/workflows/${row.original.workflowId}/chat/${row.original.instanceId}`}
                className={cn(
                  'inline-flex items-center gap-1 rounded-full border px-2 py-1 text-xs font-medium transition-colors hover:bg-warning/20',
                  statusToneClasses('warning').pill
                )}
              >
                <MessageSquare className="size-3" />
                Input
              </Link>
            </WithTooltip>
          )}
        </div>
      );
    },
  },
  {
    id: 'parentInstanceId',
    accessorKey: 'parentInstanceId',
    header: 'Parent',
    enableSorting: false,
    meta: responsiveMeta('parentInstanceId'),
    cell: ({ row }) => (
      <ParentRunLink parentInstanceId={row.original.parentInstanceId} compact />
    ),
  },
  {
    accessorKey: 'executionDurationSeconds',
    header: 'Duration',
    enableSorting: false,
    meta: responsiveMeta('executionDurationSeconds'),
    cell: ({ row }) => {
      const duration = row.original.executionDurationSeconds;
      const colorClass = getDurationColorClass(duration);

      return (
        <div className="flex items-center gap-2">
          <Zap className={`size-3.5 ${colorClass}`} />
          <span className={`text-sm font-medium tabular-nums ${colorClass}`}>
            {formatDuration(duration)}
          </span>
        </div>
      );
    },
  },
  {
    accessorKey: 'version',
    header: 'Version',
    enableSorting: false,
    meta: responsiveMeta('version'),
    cell: ({ row }) => {
      const version = row.original.version;
      return version !== undefined ? (
        <span className="inline-flex items-center rounded-full bg-muted px-2 py-0.5 text-xs font-medium text-muted-foreground">
          v{version}
        </span>
      ) : null;
    },
  },
  {
    id: 'actions',
    header: () => <span className="sr-only">Actions</span>,
    size: 100,
    meta: {
      headerClassName: 'text-right',
      cellClassName: 'text-right',
    },
    cell: ({ row }) => {
      const { instanceId, workflowId, status, hasPendingInput } = row.original;
      if (!instanceId) return null;

      const shouldShowStop = isActiveStatus(status);
      // Only a paused run needs a resume; a waiting run wakes on its own and a
      // finished run answers NotResumable.
      const shouldShowResume = canResume(row.original);
      const debugLabel = shouldShowResume
        ? 'Open in editor — resume debugging'
        : 'Open in editor';

      return (
        <div className="flex items-center justify-end gap-1 opacity-0 transition-opacity duration-150 group-hover:opacity-100">
          {status === 'suspended' && (
            <Link to={`/workflows/${workflowId}?attachInstance=${instanceId}`}>
              <WithTooltip label={debugLabel}>
                <Button
                  variant="secondary"
                  size="icon"
                  className="h-auto w-auto rounded-lg p-2 text-warning transition-colors hover:bg-warning/10 hover:text-warning"
                  aria-label={debugLabel}
                >
                  <Bug className="size-4" />
                </Button>
              </WithTooltip>
            </Link>
          )}
          {hasPendingInput && (
            <Link to={`/workflows/${workflowId}/chat/${instanceId}`}>
              <WithTooltip label="Continue chat">
                <Button
                  variant="secondary"
                  size="icon"
                  className="h-auto w-auto rounded-lg p-2 text-warning transition-colors hover:bg-warning/10 hover:text-warning"
                  aria-label="Continue chat"
                >
                  <MessageSquare className="size-4" />
                </Button>
              </WithTooltip>
            </Link>
          )}
          <Link to={`/workflows/${workflowId}/history/${instanceId}`}>
            <WithTooltip label="View details">
              <Button
                variant="secondary"
                size="icon"
                className="h-auto w-auto rounded-lg p-2 transition-colors hover:bg-primary/10"
                aria-label="View details"
              >
                <Eye className="size-4" />
              </Button>
            </WithTooltip>
          </Link>
          {shouldShowResume && (
            <ResumeButton
              instanceId={instanceId}
              variant="secondary"
              size="icon"
              className="h-auto w-auto rounded-lg p-2 text-muted-foreground transition-colors hover:bg-primary/10 hover:text-primary"
            />
          )}
          {shouldShowStop ? (
            <StopButton
              instanceId={instanceId}
              variant="secondary"
              size="icon"
              className="h-auto w-auto rounded-lg p-2 text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive"
            />
          ) : (
            <ReplayButton
              instanceId={instanceId}
              variant="secondary"
              size="icon"
              className="h-auto w-auto rounded-lg p-2 text-muted-foreground transition-colors hover:bg-success/10 hover:text-success"
            />
          )}
        </div>
      );
    },
  },
];
