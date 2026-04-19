import { useState } from 'react';
import {
  Pencil,
  Key,
  Link2,
  Mail,
  Trash2,
  Loader2,
  Webhook,
} from 'lucide-react';
import {
  getHttpTriggerUrl,
  getEmailTriggerAddress,
  getChannelWebhookUrl,
} from '@/features/triggers/utils/endpoints';
import { EnrichedTrigger } from '@/features/triggers/types';
import { Button } from '@/shared/components/ui/button.tsx';
import { Link } from 'react-router';
import { ModalDialog } from '@/shared/components/next-dialog';
import {
  DialogClose,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/shared/components/ui/dialog';
import { toast } from 'sonner';
import { EntityTile } from '@/shared/components/entity-tile';

interface TriggerCardProps {
  trigger: EnrichedTrigger;
  onDelete: (id: string) => void;
  loading?: boolean;
}

export function TriggerCard({ trigger, onDelete, loading }: TriggerCardProps) {
  const [showDeleteConfirm, setShowDeleteConfirm] = useState(false);
  const {
    id,
    workflowName,
    triggerType,
    configurationPreview,
    active,
    tenantId,
  } = trigger;

  const handleDelete = () => {
    onDelete(id);
    setShowDeleteConfirm(false);
  };

  const handleCopyId = () => {
    navigator.clipboard.writeText(id);
    toast.success('ID copied to clipboard');
  };

  const getEndpointInfo = () => {
    if (triggerType === 'HTTP' && tenantId) {
      return {
        value: getHttpTriggerUrl(id, tenantId),
        label: 'URL',
        icon: Link2,
      };
    }
    if (triggerType === 'EMAIL') {
      return {
        value: getEmailTriggerAddress(id),
        label: 'Email',
        icon: Mail,
      };
    }
    if (triggerType === 'CHANNEL') {
      const url =
        trigger.webhookUrl ||
        (tenantId &&
          (trigger.configuration as any)?.connection_id &&
          getChannelWebhookUrl(
            tenantId,
            (trigger.configuration as any).connection_id
          ));
      if (url) {
        return {
          value: url,
          label: 'Webhook',
          icon: Webhook,
        };
      }
    }
    return null;
  };

  const endpointInfo = getEndpointInfo();

  const handleCopyEndpoint = () => {
    if (endpointInfo) {
      navigator.clipboard.writeText(endpointInfo.value);
      toast.success(`${endpointInfo.label} copied to clipboard`);
    }
  };

  const getTriggerTypeLabel = (type?: string) => {
    if (!type) return 'Unknown';
    return type
      .replace(/_/g, ' ')
      .toLowerCase()
      .replace(/\b\w/g, (l) => l.toUpperCase());
  };

  return (
    <>
      <EntityTile
        kicker={getTriggerTypeLabel(triggerType)}
        title={
          <Link
            to={`/invocation-triggers/${id}`}
            className="hover:underline hover:text-primary"
          >
            {workflowName}
          </Link>
        }
        description={configurationPreview}
        metadata={[active ? 'Active' : 'Inactive', `Trigger ID • ${id}`]}
        tags={
          endpointInfo && (
            <span
              className="flex items-center gap-1.5 cursor-pointer hover:text-foreground transition-colors min-w-0"
              onClick={handleCopyEndpoint}
              title={`Click to copy ${endpointInfo.label}: ${endpointInfo.value}`}
            >
              <endpointInfo.icon size={12} className="flex-shrink-0" />
              <span className="font-mono text-[11px] truncate">
                {endpointInfo.value}
              </span>
            </span>
          )
        }
        actions={
          <>
            <Button
              variant="ghost"
              size="icon"
              onClick={handleCopyId}
              className="p-2 h-auto w-auto text-slate-400 hover:text-slate-600 hover:bg-slate-100 dark:hover:bg-slate-800 dark:hover:text-slate-300 rounded-lg transition-colors"
              title="Copy ID"
            >
              <Key className="w-4 h-4" />
            </Button>
            <Link to={`/invocation-triggers/${id}`}>
              <Button
                variant="ghost"
                size="icon"
                className="p-2 h-auto w-auto text-slate-400 hover:text-blue-600 hover:bg-blue-50 dark:hover:bg-blue-900/30 dark:hover:text-blue-400 rounded-lg transition-colors"
                title="Edit trigger"
              >
                <Pencil className="w-4 h-4" />
              </Button>
            </Link>
            <Button
              variant="ghost"
              size="icon"
              onClick={() => setShowDeleteConfirm(true)}
              className="p-2 h-auto w-auto text-slate-400 hover:text-red-600 hover:bg-red-50 dark:hover:bg-red-900/30 dark:hover:text-red-400 rounded-lg transition-colors"
              title="Delete trigger"
              disabled={loading}
            >
              {loading ? (
                <Loader2 className="w-4 h-4 animate-spin" />
              ) : (
                <Trash2 className="w-4 h-4" />
              )}
            </Button>
          </>
        }
      />

      <ModalDialog
        open={showDeleteConfirm}
        onClose={() => setShowDeleteConfirm(false)}
      >
        <DialogHeader>
          <DialogTitle>Delete Trigger</DialogTitle>
          <DialogDescription>
            Are you sure you want to delete this trigger for "{workflowName}"?
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
            disabled={loading}
          >
            {loading ? 'Deleting...' : 'Delete Trigger'}
          </Button>
        </DialogFooter>
      </ModalDialog>
    </>
  );
}
