import { ColumnDef } from '@tanstack/react-table';
import { Link } from 'react-router';
import {
  Loader2,
  ExternalLink,
  Eye,
  Zap,
  MessageSquare,
  Bug,
} from 'lucide-react';
import { ExecutionStatus } from '@/generated/RuntaraRuntimeApi';
import { ExecutionHistoryItem } from '../types';
import { formatDate } from '@/lib/utils';
import { Button } from '@/shared/components/ui/button';
import { isActiveStatus } from '@/shared/utils/status-display';
import { ReplayButton } from '@/features/workflows/components/ReplayButton';
import { ResumeButton } from '@/features/workflows/components/ResumeButton';
import { StopButton } from '@/features/workflows/components/StopButton';

// Helper to format duration
const formatDuration = (seconds: number | null | undefined): string => {
  if (seconds === null || seconds === undefined) return '-';
  const ms = seconds * 1000;
  if (ms < 1000) return `${Math.round(ms)}ms`;
  if (seconds < 60) return `${seconds.toFixed(1)}s`;
  return `${Math.floor(seconds / 60)}m ${Math.round(seconds % 60)}s`;
};

// Helper to get duration color based on time
const getDurationColorClass = (seconds: number | null | undefined): string => {
  if (seconds === null || seconds === undefined) return 'text-slate-400';
  const ms = seconds * 1000;
  if (ms < 100) return 'text-emerald-600 dark:text-emerald-400';
  if (ms < 1000) return 'text-slate-600 dark:text-slate-400';
  if (ms < 5000) return 'text-amber-600 dark:text-amber-400';
  return 'text-red-600 dark:text-red-400';
};

// Status badge component with consistent width
const StatusBadge = ({ status }: { status: string }) => {
  const baseClasses =
    'inline-flex items-center justify-center gap-1.5 min-w-[90px] px-2.5 py-1 text-xs font-medium rounded-full';

  if (status === 'Completed') {
    return (
      <span
        className={`${baseClasses} text-emerald-700 bg-emerald-50 border border-emerald-200/60 dark:text-emerald-400 dark:bg-emerald-900/30 dark:border-emerald-700/40`}
      >
        <span className="w-1.5 h-1.5 bg-emerald-500 rounded-full dark:bg-emerald-400"></span>
        Completed
      </span>
    );
  }

  if (status === 'Failed') {
    return (
      <span
        className={`${baseClasses} text-red-700 bg-red-50 border border-red-200/60 dark:text-red-400 dark:bg-red-900/30 dark:border-red-700/40`}
      >
        <span className="w-1.5 h-1.5 bg-red-500 rounded-full dark:bg-red-400"></span>
        Failed
      </span>
    );
  }

  if (status === 'Timeout') {
    return (
      <span
        className={`${baseClasses} text-amber-700 bg-amber-50 border border-amber-200/60 dark:text-amber-400 dark:bg-amber-900/30 dark:border-amber-700/40`}
      >
        <span className="w-1.5 h-1.5 bg-amber-500 rounded-full dark:bg-amber-400"></span>
        Timeout
      </span>
    );
  }

  if (status === 'Cancelled') {
    return (
      <span
        className={`${baseClasses} text-slate-700 bg-slate-100 border border-slate-200/60 dark:text-slate-400 dark:bg-slate-800 dark:border-slate-700/40`}
      >
        <span className="w-1.5 h-1.5 bg-slate-500 rounded-full dark:bg-slate-400"></span>
        Cancelled
      </span>
    );
  }

  if (status === 'Running') {
    return (
      <span
        className={`${baseClasses} text-blue-700 bg-blue-50 border border-blue-200/60 dark:text-blue-400 dark:bg-blue-900/30 dark:border-blue-700/40`}
      >
        <Loader2 className="w-3 h-3 animate-spin" />
        Running
      </span>
    );
  }

  if (status === 'Compiling') {
    return (
      <span
        className={`${baseClasses} text-violet-700 bg-violet-50 border border-violet-200/60 dark:text-violet-400 dark:bg-violet-900/30 dark:border-violet-700/40`}
      >
        <Loader2 className="w-3 h-3 animate-spin" />
        Compiling
      </span>
    );
  }

  if (status === 'Queued') {
    return (
      <span
        className={`${baseClasses} text-slate-600 bg-slate-100 border border-slate-200/60 dark:text-slate-400 dark:bg-slate-800 dark:border-slate-700/40`}
      >
        <span className="w-1.5 h-1.5 bg-slate-400 rounded-full dark:bg-slate-500"></span>
        Queued
      </span>
    );
  }

  if (status === 'suspended') {
    return (
      <span
        className={`${baseClasses} text-blue-700 bg-blue-50 border border-blue-200/60 dark:text-blue-400 dark:bg-blue-900/30 dark:border-blue-700/40`}
      >
        <span className="w-1.5 h-1.5 bg-blue-500 rounded-full animate-pulse dark:bg-blue-400"></span>
        Suspended
      </span>
    );
  }

  // Default/unknown status
  return (
    <span
      className={`${baseClasses} text-slate-700 bg-slate-100 border border-slate-200/60 dark:text-slate-400 dark:bg-slate-800 dark:border-slate-700/40`}
    >
      {status}
    </span>
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
              className="text-sm font-medium text-slate-900 hover:text-blue-600 inline-flex items-center gap-1.5 group/link dark:text-slate-100 dark:hover:text-blue-400"
            >
              {workflowName}
              <ExternalLink className="w-3 h-3 text-slate-400 group-hover/link:text-blue-500 transition-colors dark:group-hover/link:text-blue-400" />
            </Link>
          ) : (
            <span className="text-sm font-medium text-slate-400 italic dark:text-slate-500">
              Ad-hoc invocation
            </span>
          )}
          <span className="text-xs text-slate-400 font-mono dark:text-slate-500">
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
        <span className="text-sm text-slate-700 dark:text-slate-300">
          {formatDate(createdAt)}
        </span>
      );
    },
  },
  {
    accessorKey: 'completedAt',
    header: 'Completed',
    enableSorting: true,
    cell: ({ row }) => {
      const completedAt = row.original.completedAt;
      if (!completedAt) {
        return (
          <span className="text-sm text-slate-400 dark:text-slate-500">-</span>
        );
      }
      return (
        <span className="text-sm text-slate-700 dark:text-slate-300">
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
            <Link
              to={`/workflows/${row.original.workflowId}/chat/${row.original.instanceId}`}
              className="inline-flex items-center gap-1 px-2 py-1 text-xs font-medium rounded-full text-amber-700 bg-amber-50 border border-amber-200/60 hover:bg-amber-100 dark:text-amber-400 dark:bg-amber-900/30 dark:border-amber-700/40 dark:hover:bg-amber-900/50 transition-colors"
              title="Continue chat"
            >
              <MessageSquare className="w-3 h-3" />
              Input
            </Link>
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
          <Zap className={`w-3.5 h-3.5 ${colorClass}`} />
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
        <span className="inline-flex items-center px-2 py-0.5 text-xs font-medium text-slate-500 bg-slate-100 rounded dark:text-slate-400 dark:bg-slate-800">
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
        <div className="flex items-center justify-end gap-1 opacity-0 group-hover:opacity-100 transition-opacity duration-150">
          {status === ExecutionStatus.Suspended && (
            <Link to={`/workflows/${workflowId}?attachInstance=${instanceId}`}>
              <Button
                variant="ghost"
                size="icon"
                className="p-2 h-auto w-auto text-orange-500 hover:text-orange-600 hover:bg-orange-50 dark:hover:bg-orange-900/30 dark:hover:text-orange-400 rounded-lg transition-colors"
                title="Open in editor — resume debugging"
              >
                <Bug className="w-4 h-4" />
              </Button>
            </Link>
          )}
          {hasPendingInput && (
            <Link to={`/workflows/${workflowId}/chat/${instanceId}`}>
              <Button
                variant="ghost"
                size="icon"
                className="p-2 h-auto w-auto text-amber-500 hover:text-amber-600 hover:bg-amber-50 dark:hover:bg-amber-900/30 dark:hover:text-amber-400 rounded-lg transition-colors"
                title="Continue chat"
              >
                <MessageSquare className="w-4 h-4" />
              </Button>
            </Link>
          )}
          <Link to={`/workflows/${workflowId}/history/${instanceId}`}>
            <Button
              variant="ghost"
              size="icon"
              className="p-2 h-auto w-auto text-slate-400 hover:text-blue-600 hover:bg-blue-50 dark:hover:bg-blue-900/30 dark:hover:text-blue-400 rounded-lg transition-colors"
              title="View details"
            >
              <Eye className="w-4 h-4" />
            </Button>
          </Link>
          {shouldShowStop ? (
            <StopButton
              instanceId={instanceId}
              variant="ghost"
              size="icon"
              className="p-2 h-auto w-auto text-slate-400 hover:text-red-600 hover:bg-red-50 dark:hover:bg-red-900/30 dark:hover:text-red-400 rounded-lg transition-colors"
            />
          ) : (
            <>
              {(status === ExecutionStatus.Failed ||
                status === ExecutionStatus.Cancelled) && (
                <ResumeButton
                  instanceId={instanceId}
                  variant="ghost"
                  size="icon"
                  className="p-2 h-auto w-auto text-slate-400 hover:text-blue-600 hover:bg-blue-50 dark:hover:bg-blue-900/30 dark:hover:text-blue-400 rounded-lg transition-colors"
                />
              )}
              <ReplayButton
                instanceId={instanceId}
                variant="ghost"
                size="icon"
                className="p-2 h-auto w-auto text-slate-400 hover:text-emerald-600 hover:bg-emerald-50 dark:hover:bg-emerald-900/30 dark:hover:text-emerald-400 rounded-lg transition-colors"
              />
            </>
          )}
        </div>
      );
    },
  },
];
