import { useState } from 'react';
import { Link } from 'react-router';
import { toast } from 'sonner';
import { Pencil, Trash2, Loader2, Copy } from 'lucide-react';
import { EnrichedTrigger, TriggerType } from '@/features/triggers/types';
import { useCustomMutation } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys.ts';
import { queryClient } from '@/main.tsx';
import { removeInvocationTrigger } from '@/features/triggers/queries';
import {
  getHttpTriggerUrl,
  getEmailTriggerAddress,
  getChannelWebhookUrl,
} from '@/features/triggers/utils/endpoints';
import { Icons } from '@/shared/components/icons.tsx';
import { Badge } from '@/shared/components/ui/badge';
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
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
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
      (tenantId && connectionId && getChannelWebhookUrl(tenantId, connectionId)) ||
      null
    );
  }
  return null;
}

export function TriggersGrid({ data = [] }: TriggersGridProps) {
  const [deleteTarget, setDeleteTarget] = useState<EnrichedTrigger | null>(null);

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

  if (!data || data.length === 0) {
    return (
      <div className="rounded-lg border bg-muted/20 px-6 py-10 text-center">
        <Icons.inbox className="mx-auto mb-4 h-10 w-10 text-muted-foreground" />
        <p className="text-base font-semibold text-foreground">
          No triggers yet
        </p>
        <p className="mt-1 text-sm text-muted-foreground">
          Create your first trigger to connect external events.
        </p>
      </div>
    );
  }

  // Sort triggers by workflow name
  const sortedTriggers = [...data].sort((a, b) =>
    (a.workflowName || '').localeCompare(b.workflowName || '')
  );

  return (
    <TooltipProvider delayDuration={150}>
      <div className="rounded-lg border overflow-hidden">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>Name</TableHead>
              <TableHead>Type</TableHead>
              <TableHead>Status</TableHead>
              <TableHead>Endpoint</TableHead>
              <TableHead className="w-0" />
            </TableRow>
          </TableHeader>
          <TableBody>
            {sortedTriggers.map((trigger) => {
              const endpoint = getEndpoint(trigger);
              return (
                <TableRow key={trigger.id}>
                  <TableCell className="font-medium text-foreground">
                    <Link
                      to={`/invocation-triggers/${trigger.id}`}
                      className="hover:underline hover:text-primary"
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
                    <Badge variant={trigger.active ? 'success' : 'muted'}>
                      {trigger.active ? 'Active' : 'Inactive'}
                    </Badge>
                  </TableCell>
                  <TableCell className="text-muted-foreground">
                    {endpoint ? (
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
                    ) : (
                      <span className="font-mono text-xs text-muted-foreground/70">
                        {trigger.id}
                      </span>
                    )}
                  </TableCell>
                  <TableCell className="text-right">
                    <div className="flex items-center justify-end gap-1">
                      <Link to={`/invocation-triggers/${trigger.id}`}>
                        <Button
                          variant="ghost"
                          size="icon"
                          className="h-7 w-7 text-muted-foreground"
                          title="Edit trigger"
                        >
                          <Pencil className="h-4 w-4" />
                        </Button>
                      </Link>
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-7 w-7 text-muted-foreground hover:text-destructive"
                        title="Delete trigger"
                        disabled={deletingId === trigger.id}
                        onClick={() => setDeleteTarget(trigger)}
                      >
                        {deletingId === trigger.id ? (
                          <Loader2 className="h-4 w-4 animate-spin" />
                        ) : (
                          <Trash2 className="h-4 w-4" />
                        )}
                      </Button>
                    </div>
                  </TableCell>
                </TableRow>
              );
            })}
          </TableBody>
        </Table>
      </div>

      <ModalDialog
        open={!!deleteTarget}
        onClose={() => setDeleteTarget(null)}
      >
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
    </TooltipProvider>
  );
}
