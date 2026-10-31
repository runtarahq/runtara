import { ReactNode, useState } from 'react';
import { Link } from 'react-router';
import { toast } from 'sonner';
import {
  AppWindow,
  Clock,
  Copy,
  Globe,
  Mail,
  MessageSquare,
  Zap,
  type LucideIcon,
} from 'lucide-react';
import { EnrichedTrigger, TriggerType } from '@/features/triggers/types';
import { useCustomMutation } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys.ts';
import { queryClient } from '@/bootstrap.tsx';
import { removeInvocationTrigger } from '@/features/triggers/queries';
import {
  getHttpTriggerUrl,
  getHttpSyncUrl,
  getEmailTriggerAddress,
  getChannelWebhookUrl,
} from '@/features/triggers/utils/endpoints';
import { formatDate } from '@/lib/utils';
import { Button } from '@/shared/components/ui/button';
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/shared/components/ui/table';
import {
  ConsoleEmptyState,
  ConsoleErrorState,
  ConsoleTableShell,
  RowActionsMenu,
  StatusPill,
  TableSkeletonRows,
  TableStatusFooter,
} from '@/shared/components/console';
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
  WithTooltip,
} from '@/shared/components/ui/tooltip';
import { ModalDialog } from '@/shared/components/next-dialog';
import {
  DialogClose,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/shared/components/ui/dialog';

interface TriggersGridProps {
  data?: EnrichedTrigger[];
  /** Pinned console toolbar (breadcrumb + actions) from the page. */
  toolbar?: ReactNode;
  isFetching?: boolean;
  isError?: boolean;
  error?: unknown;
}

const TRIGGER_TYPES: Partial<
  Record<TriggerType, { label: string; icon: LucideIcon }>
> = {
  HTTP: { label: 'HTTP', icon: Globe },
  CRON: { label: 'Cron', icon: Clock },
  EMAIL: { label: 'Email', icon: Mail },
  APPLICATION: { label: 'Application', icon: AppWindow },
  CHANNEL: { label: 'Channel', icon: MessageSquare },
};

/** Neutral chip: the type is a category, so green stays the status's alone. */
function TriggerTypeChip({ type }: { type?: TriggerType }) {
  const known = type ? TRIGGER_TYPES[type] : undefined;
  const Icon = known?.icon ?? Zap;
  return (
    <span className="inline-flex items-center gap-1.5 whitespace-nowrap rounded-md border bg-muted py-0.5 pl-1.5 pr-2 text-xs font-medium text-foreground/80">
      <Icon className="size-3.5 text-muted-foreground" aria-hidden="true" />
      {known?.label ?? getTriggerTypeLabel(type)}
    </span>
  );
}

function getTriggerTypeLabel(type?: TriggerType): string {
  if (!type) return 'Unknown';
  return type
    .replace(/_/g, ' ')
    .toLowerCase()
    .replace(/\b\w/g, (l) => l.toUpperCase());
}

function getEndpoint(trigger: EnrichedTrigger): string | null {
  const { id, triggerType, tenantId } = trigger;
  if (triggerType === 'HTTP' && tenantId) {
    return getHttpTriggerUrl(id, tenantId);
  }
  if (triggerType === 'EMAIL') {
    return getEmailTriggerAddress(id);
  }
  if (triggerType === 'CHANNEL') {
    const connectionId = (trigger.configuration as any)?.connection_id;
    return (
      trigger.webhookUrl ||
      (tenantId &&
        connectionId &&
        getChannelWebhookUrl(tenantId, connectionId)) ||
      null
    );
  }
  return null;
}

/** Synchronous-execution endpoint variant; HTTP triggers only. */
function getSyncEndpoint(trigger: EnrichedTrigger): string | null {
  const { triggerType, tenantId, workflowId } = trigger;
  if (triggerType === 'HTTP' && tenantId && workflowId) {
    return getHttpSyncUrl(workflowId, tenantId);
  }
  return null;
}

function formatLastRun(lastRun?: string | null): string {
  if (!lastRun) {
    return '—';
  }
  const date = new Date(lastRun);
  if (isNaN(date.getTime())) {
    return '—';
  }
  return formatDate(date);
}

export function TriggersGrid({
  data = [],
  toolbar,
  isFetching = false,
  isError = false,
  error,
}: TriggersGridProps) {
  const [deleteTarget, setDeleteTarget] = useState<EnrichedTrigger | null>(
    null
  );

  const removeMutation = useCustomMutation({
    mutationFn: removeInvocationTrigger,
    onSuccess: () => {
      toast.info('Invocation Trigger has been removed');
      queryClient.invalidateQueries({
        queryKey: queryKeys.triggers.all,
      });
    },
    onSettled: () => {
      setDeleteTarget(null);
    },
  });

  const deletingId = removeMutation.isPending ? deleteTarget?.id : null;

  const handleDelete = () => {
    if (deleteTarget) {
      removeMutation.mutate(deleteTarget.id);
    }
  };

  const handleCopyEndpoint = (endpoint: string) => {
    navigator.clipboard.writeText(endpoint);
    toast.success('Endpoint copied to clipboard');
  };

  // Sort triggers by workflow name
  const sortedTriggers = [...data].sort((a, b) =>
    (a.workflowName || '').localeCompare(b.workflowName || '')
  );

  const hasContent = sortedTriggers.length > 0;

  let body: ReactNode;
  if (isFetching) {
    body = (
      <TableSkeletonRows
        rows={8}
        widths={['w-40', 'w-16', 'w-16', 'ml-auto w-48']}
      />
    );
  } else if (isError) {
    body = <ConsoleErrorState error={error} entityLabel="triggers" />;
  } else if (!hasContent) {
    body = (
      <ConsoleEmptyState
        title="No triggers yet"
        description="Create your first trigger to connect external events."
      />
    );
  } else {
    body = (
      <TooltipProvider delayDuration={150}>
        <Table variant="console" className="min-w-[860px] table-fixed">
          <TableHeader>
            <TableRow>
              <TableHead>Name</TableHead>
              <TableHead className="w-36">Type</TableHead>
              <TableHead className="w-32">Status</TableHead>
              <TableHead className="w-48">Last run</TableHead>
              <TableHead className="w-[36%]">Endpoint</TableHead>
              <TableHead className="w-16">
                <span className="sr-only">Actions</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {sortedTriggers.map((trigger) => {
              const endpoint = getEndpoint(trigger);
              const syncEndpoint = getSyncEndpoint(trigger);
              return (
                <TableRow key={trigger.id}>
                  <TableCell className="font-medium text-foreground">
                    <Link
                      to={`/invocation-triggers/${trigger.id}`}
                      className="block truncate hover:text-primary-text hover:underline"
                      title={trigger.workflowName}
                    >
                      {trigger.workflowName}
                    </Link>
                  </TableCell>
                  <TableCell>
                    <TriggerTypeChip type={trigger.triggerType} />
                  </TableCell>
                  <TableCell>
                    <StatusPill
                      tone={trigger.active ? 'success' : 'neutral'}
                      label={trigger.active ? 'Active' : 'Inactive'}
                    />
                  </TableCell>
                  <TableCell
                    className={
                      trigger.lastRun
                        ? 'whitespace-nowrap tabular-nums text-muted-foreground'
                        : 'text-muted-foreground/60'
                    }
                  >
                    {formatLastRun(trigger.lastRun)}
                  </TableCell>
                  <TableCell>
                    {endpoint ? (
                      <div className="flex min-w-0 items-center gap-2">
                        <Tooltip>
                          <TooltipTrigger asChild>
                            <span className="min-w-0 truncate font-mono text-[12.5px] text-muted-foreground">
                              {endpoint}
                            </span>
                          </TooltipTrigger>
                          <TooltipContent className="max-w-[36rem] break-all font-mono text-xs">
                            {endpoint}
                          </TooltipContent>
                        </Tooltip>
                        {/* The sync variant is a different URL; the tag is
                            how it is copied now that it has no line of its
                            own. */}
                        {syncEndpoint && (
                          <WithTooltip label="Copy sync endpoint — waits up to 30 s for the result and keeps no run history">
                            <button
                              type="button"
                              aria-label="Copy sync endpoint"
                              onClick={() => handleCopyEndpoint(syncEndpoint)}
                              className="shrink-0 whitespace-nowrap rounded-[5px] border px-1.5 py-px text-2xs font-medium text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                            >
                              Sync · 30s · no history
                            </button>
                          </WithTooltip>
                        )}
                        <WithTooltip label="Copy endpoint">
                          <Button
                            variant="secondary"
                            size="icon-sm"
                            className="shrink-0"
                            aria-label="Copy endpoint"
                            onClick={() => handleCopyEndpoint(endpoint)}
                          >
                            <Copy className="size-3.5" />
                          </Button>
                        </WithTooltip>
                      </div>
                    ) : (
                      <span className="block truncate font-mono text-[12.5px] text-muted-foreground">
                        {trigger.id}
                      </span>
                    )}
                  </TableCell>
                  <TableCell className="text-right">
                    <RowActionsMenu
                      label={trigger.workflowName || 'trigger'}
                      editTo={`/invocation-triggers/${trigger.id}`}
                      editPermission="trigger:update"
                      onDelete={() => setDeleteTarget(trigger)}
                      deletePermission="trigger:delete"
                      deleting={deletingId === trigger.id}
                    />
                  </TableCell>
                </TableRow>
              );
            })}
          </TableBody>
        </Table>
      </TooltipProvider>
    );
  }

  return (
    <>
      <ConsoleTableShell
        toolbar={toolbar}
        footer={
          hasContent && !isFetching && !isError ? (
            <TableStatusFooter
              left={
                <span>
                  <strong className="font-semibold text-foreground">
                    {sortedTriggers.length}
                  </strong>{' '}
                  trigger{sortedTriggers.length === 1 ? '' : 's'}
                </span>
              }
            />
          ) : undefined
        }
      >
        {body}
      </ConsoleTableShell>

      <ModalDialog open={!!deleteTarget} onClose={() => setDeleteTarget(null)}>
        <DialogHeader>
          <DialogTitle>Delete Trigger</DialogTitle>
          <DialogDescription>
            Are you sure you want to delete this trigger for "
            {deleteTarget?.workflowName}"?
          </DialogDescription>
        </DialogHeader>
        <div className="py-2">
          This action cannot be undone and will stop the trigger from invoking
          the workflow.
        </div>
        <DialogFooter className="gap-2 sm:gap-0">
          <DialogClose asChild>
            <Button type="button" variant="secondary" bordered>
              Cancel
            </Button>
          </DialogClose>
          <Button
            type="button"
            variant="destructive"
            onClick={handleDelete}
            disabled={removeMutation.isPending}
          >
            {removeMutation.isPending ? 'Deleting...' : 'Delete Trigger'}
          </Button>
        </DialogFooter>
      </ModalDialog>
    </>
  );
}
