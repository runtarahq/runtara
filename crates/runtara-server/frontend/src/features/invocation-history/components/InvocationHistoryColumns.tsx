import { ColumnDef } from '@tanstack/react-table';
import { Link } from 'react-router';
import { ExternalLink, Eye, Zap, MessageSquare, Bug } from 'lucide-react';
import { ExecutionHistoryItem } from '../types';
import { cn, formatDate } from '@/lib/utils';
import { Button } from '@/shared/components/ui/button';
import { WithTooltip } from '@/shared/components/ui/tooltip';
import {
  StatusPill,
  executionStatusPill,
  statusToneClasses,
} from '@/shared/components/console';
import { isActiveStatus } from '@/shared/utils/status-display';
import { ReplayButton } from '@/features/workflows/components/ReplayButton';
import { ResumeButton } from '@/features/workflows/components/ResumeButton';
import { StopButton } from '@/features/workflows/components/StopButton';

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

// Status badge — delegates to the shared console StatusPill with a consistent width
const StatusBadge = ({ status }: { status: string }) => {
  const { tone, label, spin, pulse } = executionStatusPill(status);
  return (
    <StatusPill
      tone={tone}
      label={label}
      spin={spin}
      pulse={pulse}
      className="min-w-[90px]"
    />
  );
};

export const invocationHistoryColumns: ColumnDef<ExecutionHistoryItem>[] = [
  {
    id: 'workflowId',
    accessorKey: 'workflowName',
    header: 'Workflow',
    enableSorting: false,
    cell: ({ row }) => {
      const workflowId = row.original.workflowId;
      const workflowName = row.original.workflowName || workflowId;
      const instanceId = row.original.instanceId;

      return (
        <div className="flex flex-col gap-0.5">
          {workflowName ? (
            <Link
              to={`/workflows/${workflowId}`}
              className="group/link inline-flex items-center gap-1.5 text-sm font-medium text-foreground hover:text-primary"
            >
              {workflowName}
              <ExternalLink className="size-3 text-muted-foreground transition-colors group-hover/link:text-primary" />
            </Link>
          ) : (
            <span className="text-sm font-medium italic text-muted-foreground">
              Ad-hoc invocation
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
      return (
        <span className="text-sm text-foreground">{formatDate(createdAt)}</span>
      );
    },
  },
  {
    accessorKey: 'completedAt',
    header: 'Completed',
    enableSorting: true,
    cell: ({ row }) => {
      const completedAt = row.original.completedAt;
      // A non-terminal row (running/suspended/…) has no real completion time;
      // its `completedAt` is a suspend/drain timestamp, so don't present it as
      // "Completed".
      if (!completedAt || isActiveStatus(row.original.status)) {
        return <span className="text-sm text-muted-foreground">-</span>;
      }
      return (
        <span className="text-sm text-foreground">
          {formatDate(completedAt)}
        </span>
      );
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
          <StatusBadge status={status} />
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
    accessorKey: 'executionDurationSeconds',
    header: 'Duration',
    enableSorting: false,
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
    cell: ({ row }) => {
      const version = row.original.version;
      return version !== undefined ? (
        <span className="inline-flex items-center rounded bg-muted px-2 py-0.5 text-xs font-medium text-muted-foreground">
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

      return (
        <div className="flex items-center justify-end gap-1 opacity-0 transition-opacity duration-150 group-hover:opacity-100">
          {status === 'suspended' && (
            <Link to={`/workflows/${workflowId}?attachInstance=${instanceId}`}>
              <WithTooltip label="Open in editor — resume debugging">
                <Button
                  variant="ghost"
                  size="icon"
                  className="h-auto w-auto rounded-lg p-2 text-warning transition-colors hover:bg-warning/10 hover:text-warning"
                  aria-label="Open in editor — resume debugging"
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
                  variant="ghost"
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
                variant="ghost"
                size="icon"
                className="h-auto w-auto rounded-lg p-2 text-muted-foreground transition-colors hover:bg-primary/10 hover:text-primary"
                aria-label="View details"
              >
                <Eye className="size-4" />
              </Button>
            </WithTooltip>
          </Link>
          {shouldShowStop ? (
            <StopButton
              instanceId={instanceId}
              variant="ghost"
              size="icon"
              className="h-auto w-auto rounded-lg p-2 text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive"
            />
          ) : (
            <>
              {(status === 'failed' || status === 'cancelled') && (
                <ResumeButton
                  instanceId={instanceId}
                  variant="ghost"
                  size="icon"
                  className="h-auto w-auto rounded-lg p-2 text-muted-foreground transition-colors hover:bg-primary/10 hover:text-primary"
                />
              )}
              <ReplayButton
                instanceId={instanceId}
                variant="ghost"
                size="icon"
                className="h-auto w-auto rounded-lg p-2 text-muted-foreground transition-colors hover:bg-success/10 hover:text-success"
              />
            </>
          )}
        </div>
      );
    },
  },
];
