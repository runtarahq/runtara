import { Link } from 'react-router';
import { GitBranch } from 'lucide-react';
import { cn, formatDate } from '@/lib/utils';
import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { StatusPill, executionStatusPill } from '@/shared/components/console';
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from '@/shared/components/ui/card';
import type { SuspensionReason } from '@/generated/RuntaraRuntimeApi';
import { executionDisplayName } from '@/features/workflows/utils/run-label';
import { suspendedStatusLabel } from '@/features/workflows/utils/suspension';
import { getAllExecutions } from '../queries';
import {
  childRunsListPath,
  childRunsQueryParams,
  findRun,
  runDetailPath,
  runLookupQueryParams,
} from '../utils/run-links';

type ExecutionsPage = Awaited<ReturnType<typeof getAllExecutions>>;

/**
 * Execution status pill; a suspended run shows why it is suspended (Paused,
 * Waiting for signal, ...) instead of a bare "Suspended".
 */
export function RunStatusPill({
  status,
  suspensionReason,
  className,
}: {
  status: string;
  suspensionReason?: SuspensionReason | null;
  className?: string;
}) {
  const { tone, label, spin, pulse } = executionStatusPill(status);
  const reasonLabel = suspendedStatusLabel({ status, suspensionReason });
  return (
    <span title={reasonLabel ? `Suspended — ${reasonLabel}` : undefined}>
      <StatusPill
        tone={tone}
        label={reasonLabel ?? label}
        spin={spin}
        pulse={pulse}
        className={className}
      />
    </span>
  );
}

/**
 * Link to the run whose `control:start` step started this one. The detail
 * route needs the parent's workflow, so the parent is looked up by id; until
 * then (or when it is gone) the bare id is shown.
 */
export function ParentRunLink({
  parentInstanceId,
  className,
}: {
  parentInstanceId?: string | null;
  className?: string;
}) {
  const { data } = useCustomQuery<ExecutionsPage>({
    queryKey: queryKeys.executions.list(
      runLookupQueryParams(parentInstanceId ?? '')
    ),
    queryFn: getAllExecutions,
    enabled: !!parentInstanceId,
    staleTime: 60_000,
  });

  if (!parentInstanceId) {
    return <span className="text-sm text-muted-foreground">—</span>;
  }

  const parent = findRun(data?.content, parentInstanceId);
  if (!parent?.workflowId) {
    return (
      <span
        className={cn('font-mono text-xs text-muted-foreground', className)}
        title={parentInstanceId}
      >
        {parentInstanceId}
      </span>
    );
  }

  return (
    <Link
      to={runDetailPath(parent.workflowId, parentInstanceId)}
      className={cn(
        'inline-flex min-w-0 flex-col text-sm text-foreground hover:text-primary',
        className
      )}
      title={parentInstanceId}
    >
      <span className="max-w-60 truncate font-medium">
        {executionDisplayName(parent)}
      </span>
      <span className="max-w-60 truncate font-mono text-xs text-muted-foreground">
        {parentInstanceId}
      </span>
    </Link>
  );
}

/**
 * The runs this run started through `control:start`. Hidden when there are
 * none — most runs start nothing.
 */
export function ChildRunsCard({ instanceId }: { instanceId: string }) {
  const { data } = useCustomQuery<ExecutionsPage>({
    queryKey: queryKeys.executions.list(childRunsQueryParams(instanceId)),
    queryFn: getAllExecutions,
    enabled: !!instanceId,
    staleTime: 0,
    // Never show another run's children while this run's page loads.
    placeholderData: undefined,
  });

  const children = data?.content ?? [];
  if (children.length === 0) return null;
  const total = data?.totalElements ?? children.length;

  return (
    <Card data-testid="child-runs">
      <CardHeader>
        <CardTitle className="flex items-center justify-between gap-2">
          <span className="flex items-center gap-2">
            <GitBranch className="size-5" />
            Child runs
            <span className="text-sm font-normal text-muted-foreground">
              ({total})
            </span>
          </span>
          {total > children.length && (
            <Link
              to={childRunsListPath(instanceId)}
              className="text-sm font-normal text-primary hover:underline"
            >
              View all
            </Link>
          )}
        </CardTitle>
      </CardHeader>
      <CardContent className="divide-y">
        {children.map((child) => (
          <div
            key={child.instanceId}
            className="flex items-center justify-between gap-4 py-2"
          >
            <Link
              to={runDetailPath(child.workflowId, child.instanceId)}
              className="flex min-w-0 flex-col hover:text-primary"
            >
              <span className="truncate text-sm font-medium">
                {executionDisplayName(child)}
              </span>
              <span className="truncate font-mono text-xs text-muted-foreground">
                {child.instanceId}
              </span>
            </Link>
            <div className="flex shrink-0 items-center gap-3">
              <span className="text-xs text-muted-foreground">
                {formatDate(child.createdAt)}
              </span>
              <RunStatusPill
                status={child.status}
                suspensionReason={child.suspensionReason}
                className="min-w-[90px]"
              />
            </div>
          </div>
        ))}
      </CardContent>
    </Card>
  );
}
