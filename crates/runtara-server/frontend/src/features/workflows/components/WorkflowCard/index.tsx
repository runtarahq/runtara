import {
  Play,
  Pencil,
  Copy,
  Trash2,
  AlertCircle,
  FolderInput,
  MessageSquare,
} from 'lucide-react';
import { WorkflowDto } from '@/generated/RuntaraRuntimeApi';
import { cn, formatDate, formatRelativeTime } from '@/lib/utils.ts';
import { Button } from '@/shared/components/ui/button.tsx';
import { Can } from '@/shared/components/Can';
import { TableCell, TableRow } from '@/shared/components/ui/table';
import { parseSchema } from '@/features/workflows/utils/schema';
import { Spinner } from '@/shared/components/ui/spinner';

interface WorkflowCardProps {
  workflow: WorkflowDto;
  onUpdate: (workflow: WorkflowDto) => void;
  onDelete: (workflow: WorkflowDto) => void;
  onSchedule: (workflow: WorkflowDto) => void;
  onClone: (workflow: WorkflowDto) => void;
  onChat?: (workflow: WorkflowDto) => void;
  onMoveToFolder?: (workflow: WorkflowDto) => void;
  pendingActionId?: string;
  pendingActionType?: 'schedule' | 'clone' | 'delete' | 'move';
  className?: string;
  /** Whether to show the move to folder button */
  showMoveAction?: boolean;
}

export function WorkflowCard({
  workflow,
  onUpdate,
  onDelete,
  onSchedule,
  onClone,
  onChat,
  onMoveToFolder,
  pendingActionId,
  pendingActionType,
  className,
  showMoveAction = false,
}: WorkflowCardProps) {
  const version = workflow.currentVersionNumber;
  const isPendingThisCard = pendingActionId === workflow.id;
  const isScheduling = isPendingThisCard && pendingActionType === 'schedule';
  const isCloning = isPendingThisCard && pendingActionType === 'clone';
  const isDeleting = isPendingThisCard && pendingActionType === 'delete';
  const isMoving = isPendingThisCard && pendingActionType === 'move';
  const description = workflow.description?.trim();
  const title = workflow.name || 'Untitled workflow';

  const rawSchema =
    (workflow as any).inputSchema ?? (workflow as any).input_schema ?? {};
  const hasInputs = parseSchema(rawSchema).length > 0;
  // The server answers whether this workflow has a step that waits for a reply.
  // Without one the chat surface accepts messages nothing ever reads, so the
  // row does not offer the action at all.
  const supportsChat = workflow.supportsChat === true;

  return (
    <TableRow className={cn('group', className)}>
      <TableCell className="font-medium text-foreground">
        <div className="flex items-center gap-2">
          <span className="shrink-0 rounded-full bg-muted px-1.5 py-0.5 text-3xs font-medium text-muted-foreground">
            {version !== undefined ? `v${version}` : 'Draft'}
          </span>
          <button
            type="button"
            onClick={() => onUpdate(workflow)}
            className="min-w-0 flex-1 truncate text-left transition-colors hover:text-primary"
          >
            {title}
          </button>
          {hasInputs && (
            <span className="inline-flex shrink-0 items-center gap-1 rounded-full border border-warning/30 bg-warning/10 px-1.5 py-0.5 text-3xs font-medium text-warning">
              <AlertCircle className="size-2.5" />
              Inputs
            </span>
          )}
        </div>
      </TableCell>
      <TableCell className="text-muted-foreground">
        <div className="truncate">
          {description || <span className="text-muted-foreground/60">—</span>}
        </div>
      </TableCell>
      <TableCell
        className="whitespace-nowrap text-muted-foreground"
        title={
          workflow.updated ? formatRelativeTime(workflow.updated) : undefined
        }
      >
        {workflow.updated ? formatDate(workflow.updated) : '—'}
      </TableCell>
      <TableCell className="text-right">
        <div className="flex items-center justify-end gap-1 opacity-0 transition-opacity focus-within:opacity-100 group-hover:opacity-100">
          <Can permission="workflow:execute">
            <Button
              variant="secondary"
              size="icon-sm"
              onClick={() => onSchedule(workflow)}
              title="Start"

              disabled={isScheduling}
            >
              {isScheduling ? (
                <Spinner className="size-4" />
              ) : (
                <Play className="size-4" />
              )}
            </Button>
          </Can>
          {onChat && supportsChat && (
            // Opening chat queues a run, so it is execute-class like Start —
            // the server maps POST /workflows/{id}/sessions to the same
            // permission.
            <Can permission="workflow:execute">
              <Button
                variant="secondary"
                size="icon-sm"
                onClick={() => onChat(workflow)}
                title="Chat"
              >
                <MessageSquare className="size-4" />
              </Button>
            </Can>
          )}
          <Can permission="workflow:update">
            <Button
              variant="secondary"
              size="icon-sm"
              onClick={() => onUpdate(workflow)}
              title="Edit"
            >
              <Pencil className="size-4" />
            </Button>
          </Can>
          {showMoveAction && onMoveToFolder && (
            <Can permission="workflow:update">
              <Button
                variant="secondary"
                size="icon-sm"
                onClick={() => onMoveToFolder(workflow)}
                title="Move to folder"

                disabled={isMoving}
              >
                {isMoving ? (
                  <Spinner className="size-4" />
                ) : (
                  <FolderInput className="size-4" />
                )}
              </Button>
            </Can>
          )}
          <Can permission="workflow:create">
            <Button
              variant="secondary"
              size="icon-sm"
              onClick={() => onClone(workflow)}
              title="Duplicate"

              disabled={isCloning}
            >
              {isCloning ? (
                <Spinner className="size-4" />
              ) : (
                <Copy className="size-4" />
              )}
            </Button>
          </Can>
          <Can permission="workflow:delete">
            <Button
              variant="secondaryDestructive"
              size="icon-sm"
              onClick={() => onDelete(workflow)}
              title="Delete"

              disabled={isDeleting}
            >
              {isDeleting ? (
                <Spinner className="size-4" />
              ) : (
                <Trash2 className="size-4" />
              )}
            </Button>
          </Can>
        </div>
      </TableCell>
    </TableRow>
  );
}
