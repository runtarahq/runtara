import { CSSProperties } from 'react';
import {
  Loader2,
  Play,
  Pencil,
  Copy,
  Trash2,
  AlertCircle,
  FolderInput,
  MessageSquare,
} from 'lucide-react';
import { WorkflowDto } from '@/generated/RuntaraRuntimeApi';
import { cn, formatDate } from '@/lib/utils.ts';
import { Button } from '@/shared/components/ui/button.tsx';
import { TableCell, TableRow } from '@/shared/components/ui/table';
import { parseSchema } from '@/features/workflows/utils/schema';

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
  style?: CSSProperties;
  /** Whether to show the move to folder button */
  showMoveAction?: boolean;
}

const getRelativeTime = (date: string | undefined) => {
  if (!date) return 'unknown time';

  const now = new Date();
  const past = new Date(date);
  const diffMs = now.getTime() - past.getTime();
  const diffMins = Math.floor(diffMs / 60000);
  const diffHours = Math.floor(diffMs / 3600000);
  const diffDays = Math.floor(diffMs / 86400000);

  if (diffMins < 1) return 'just now';
  if (diffMins < 60) return `${diffMins} min ago`;
  if (diffHours < 24) return `${diffHours} hr ago`;
  if (diffDays < 7) return `${diffDays} days ago`;
  return formatDate(date);
};

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
  style,
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

  return (
    <TableRow className={cn('group', className)} style={style}>
      <TableCell className="font-medium text-foreground">
        <div className="flex items-center gap-2">
          <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] font-medium text-muted-foreground">
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
            <span className="inline-flex shrink-0 items-center gap-1 rounded border border-warning/30 bg-warning/10 px-1.5 py-0.5 text-[10px] font-medium text-warning">
              <AlertCircle className="h-2.5 w-2.5" />
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
      <TableCell className="whitespace-nowrap text-muted-foreground">
        {workflow.updated ? getRelativeTime(workflow.updated) : '—'}
      </TableCell>
      <TableCell className="text-right">
        <div className="flex items-center justify-end gap-1 opacity-0 transition-opacity focus-within:opacity-100 group-hover:opacity-100">
          <Button
            variant="ghost"
            size="icon"
            onClick={() => onSchedule(workflow)}
            title="Start"
            className="h-7 w-7 text-muted-foreground"
            disabled={isScheduling}
          >
            {isScheduling ? (
              <Loader2 className="w-4 h-4 animate-spin" />
            ) : (
              <Play className="w-4 h-4" />
            )}
          </Button>
          {onChat && (
            <Button
              variant="ghost"
              size="icon"
              onClick={() => onChat(workflow)}
              title="Chat"
              className="h-7 w-7 text-muted-foreground"
            >
              <MessageSquare className="w-4 h-4" />
            </Button>
          )}
          <Button
            variant="ghost"
            size="icon"
            onClick={() => onUpdate(workflow)}
            title="Edit"
            className="h-7 w-7 text-muted-foreground"
          >
            <Pencil className="w-4 h-4" />
          </Button>
          {showMoveAction && onMoveToFolder && (
            <Button
              variant="ghost"
              size="icon"
              onClick={() => onMoveToFolder(workflow)}
              title="Move to folder"
              className="h-7 w-7 text-muted-foreground"
              disabled={isMoving}
            >
              {isMoving ? (
                <Loader2 className="w-4 h-4 animate-spin" />
              ) : (
                <FolderInput className="w-4 h-4" />
              )}
            </Button>
          )}
          <Button
            variant="ghost"
            size="icon"
            onClick={() => onClone(workflow)}
            title="Duplicate"
            className="h-7 w-7 text-muted-foreground"
            disabled={isCloning}
          >
            {isCloning ? (
              <Loader2 className="w-4 h-4 animate-spin" />
            ) : (
              <Copy className="w-4 h-4" />
            )}
          </Button>
          <Button
            variant="ghost"
            size="icon"
            onClick={() => onDelete(workflow)}
            title="Delete"
            className="h-7 w-7 text-muted-foreground hover:text-destructive"
            disabled={isDeleting}
          >
            {isDeleting ? (
              <Loader2 className="w-4 h-4 animate-spin" />
            ) : (
              <Trash2 className="w-4 h-4" />
            )}
          </Button>
        </div>
      </TableCell>
    </TableRow>
  );
}
