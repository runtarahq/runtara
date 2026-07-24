import { ReactNode, useState } from 'react';
import { Link } from 'react-router';
import { toast } from 'sonner';
import { Pencil, Trash2, Copy } from 'lucide-react';
import { EnrichedTrigger, TriggerType } from '@/features/triggers/types';
import { useCustomMutation } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys.ts';
import { queryClient } from '@/main.tsx';
import { removeInvocationTrigger } from '@/features/triggers/queries';
import {
  getHttpTriggerUrl,
  getHttpSyncUrl,
  getEmailTriggerAddress,
  getChannelWebhookUrl,
} from '@/features/triggers/utils/endpoints';
import { Badge } from '@/shared/components/ui/badge';
import { Button } from '@/shared/components/ui/button';
import { Can } from '@/shared/components/Can';
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
  StatusPill,
  TableSkeletonRows,
  TableStatusFooter,
} from '@/shared/components/console';
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from '@/shared/components/ui/tooltip';
import { ModalDialog } from '@/shared/components/next-dialog';
import { Spinner } from '@/shared/components/ui/spinner';
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
  return date.toLocaleString();
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
        <Table variant="console">
          <TableHeader>
            <TableRow>
              <TableHead>Name</TableHead>
              <TableHead>Type</TableHead>
              <TableHead>Status</TableHead>
              <TableHead>Last run</TableHead>
              <TableHead>Endpoint</TableHead>
              <TableHead className="w-0" />
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
                      className="hover:text-primary hover:underline"
                    >
                      {trigger.workflowName}
                    </Link>
                  </TableCell>
                  <TableCell className="text-muted-foreground">
                    <Badge variant="secondary">
                      {getTriggerTypeLabel(trigger.triggerType)}
                    </Badge>
                  </TableCell>
                  <TableCell className="text-muted-foreground">
                    <StatusPill
                      tone={trigger.active ? 'success' : 'neutral'}
                      label={trigger.active ? 'Active' : 'Inactive'}
                    />
                  </TableCell>
                  <TableCell className="whitespace-nowrap text-xs text-muted-foreground">
                    {formatLastRun(trigger.lastRun)}
                  </TableCell>
                  <TableCell className="text-muted-foreground">
                    {endpoint ? (
                      <div className="space-y-1">
                        <div className="flex items-center gap-1">
                          <Tooltip>
                            <TooltipTrigger asChild>
                              <span className="block max-w-[16rem] truncate font-mono text-xs text-muted-foreground">
                                {endpoint}
                              </span>
                            </TooltipTrigger>
                            <TooltipContent className="max-w-[36rem] break-all font-mono text-xs">
                              {endpoint}
                            </TooltipContent>
                          </Tooltip>
                          <Button
                            variant="ghost"
                            size="icon"
                            className="h-6 w-6 shrink-0 text-muted-foreground"
                            title="Copy endpoint"
                            onClick={() => handleCopyEndpoint(endpoint)}
                          >
                            <Copy className="h-3.5 w-3.5" />
                          </Button>
                        </div>
                        {syncEndpoint && (
                          <div className="flex items-center gap-1">
                            <span className="shrink-0 text-3xs font-medium uppercase tracking-wide text-muted-foreground/70">
                              Sync (30s, no history)
                            </span>
                            <Tooltip>
                              <TooltipTrigger asChild>
                                <span className="block max-w-[16rem] truncate font-mono text-xs text-muted-foreground">
                                  {syncEndpoint}
                                </span>
                              </TooltipTrigger>
                              <TooltipContent className="max-w-[36rem] break-all font-mono text-xs">
                                {syncEndpoint}
                              </TooltipContent>
                            </Tooltip>
                            <Button
                              variant="ghost"
                              size="icon"
                              className="h-6 w-6 shrink-0 text-muted-foreground"
                              title="Copy sync endpoint"
                              onClick={() => handleCopyEndpoint(syncEndpoint)}
                            >
                              <Copy className="h-3.5 w-3.5" />
                            </Button>
                          </div>
                        )}
                      </div>
                    ) : (
                      <span className="font-mono text-xs text-muted-foreground/70">
                        {trigger.id}
                      </span>
                    )}
                  </TableCell>
                  <TableCell className="text-right">
                    <div className="flex items-center justify-end gap-1">
                      <Can permission="trigger:update">
                        <Link to={`/invocation-triggers/${trigger.id}`}>
                          <Button
                            variant="ghost"
                            size="icon-sm"
                            className="text-muted-foreground"
                            title="Edit trigger"
                          >
                            <Pencil className="h-4 w-4" />
                          </Button>
                        </Link>
                      </Can>
                      <Can permission="trigger:delete">
                        <Button
                          variant="ghost"
                          size="icon-sm"
                          className="text-muted-foreground hover:text-destructive"
                          title="Delete trigger"
                          disabled={deletingId === trigger.id}
                          onClick={() => setDeleteTarget(trigger)}
                        >
                          {deletingId === trigger.id ? (
                            <Spinner className="h-4 w-4" />
                          ) : (
                            <Trash2 className="h-4 w-4" />
                          )}
                        </Button>
                      </Can>
                    </div>
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
              left={`${sortedTriggers.length} trigger${
                sortedTriggers.length === 1 ? '' : 's'
              }`}
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
            <Button type="button" variant="outline">
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
