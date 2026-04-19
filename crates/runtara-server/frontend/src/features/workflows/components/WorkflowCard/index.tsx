import { CSSProperties } from 'react';
import {
  Loader2,
  Play,
  Pencil,
  Copy,
  Trash2,
  Clock,
  Calendar,
  AlertCircle,
  FolderInput,
  MessageSquare,
} from 'lucide-react';
import { WorkflowDto } from '@/generated/RuntaraRuntimeApi';
import { cn, formatDate } from '@/lib/utils.ts';
import { Button } from '@/shared/components/ui/button.tsx';
import { EntityTile } from '@/shared/components/entity-tile';
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

  const metadata = [
    workflow.updated ? (
      <>
        <Clock className="w-3.5 h-3.5" />
        {getRelativeTime(workflow.updated)}
      </>
    ) : null,
    workflow.created ? (
      <>
        <Calendar className="w-3.5 h-3.5" />
        {formatDate(workflow.created)}
      </>
    ) : null,
  ].filter((item): item is JSX.Element => Boolean(item));

  return (
    <EntityTile
      className={cn(className)}
      style={style}
      kicker={version !== undefined ? `v${version}` : 'Draft'}
      title={
        <span
          className="cursor-pointer transition hover:text-slate-700 dark:hover:text-slate-200"
          onClick={() => onUpdate(workflow)}
        >
          {title}
        </span>
      }
      badges={
        hasInputs ? (
          <span className="inline-flex items-center gap-1 px-1.5 py-0.5 text-[10px] font-medium text-amber-700 bg-amber-50 rounded border border-amber-200/60 dark:bg-amber-900/30 dark:text-amber-400 dark:border-amber-700/40">
            <AlertCircle className="w-2.5 h-2.5" />
            Inputs
          </span>
        ) : undefined
      }
      description={description}
      metadata={metadata}
      actions={
        <>
          <Button
            variant="ghost"
            size="icon"
            onClick={() => onSchedule(workflow)}
            title="Start"
            className="p-2 h-auto w-auto text-slate-400 hover:text-emerald-600 hover:bg-emerald-50 dark:hover:bg-emerald-900/30 dark:hover:text-emerald-400 rounded-lg transition-colors"
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
              className="p-2 h-auto w-auto text-slate-400 hover:text-violet-600 hover:bg-violet-50 dark:hover:bg-violet-900/30 dark:hover:text-violet-400 rounded-lg transition-colors"
            >
              <MessageSquare className="w-4 h-4" />
            </Button>
          )}
          <Button
            variant="ghost"
            size="icon"
            onClick={() => onUpdate(workflow)}
            title="Edit"
            className="p-2 h-auto w-auto text-slate-400 hover:text-blue-600 hover:bg-blue-50 dark:hover:bg-blue-900/30 dark:hover:text-blue-400 rounded-lg transition-colors"
          >
            <Pencil className="w-4 h-4" />
          </Button>
          {showMoveAction && onMoveToFolder && (
            <Button
              variant="ghost"
              size="icon"
              onClick={() => onMoveToFolder(workflow)}
              title="Move to folder"
              className="p-2 h-auto w-auto text-slate-400 hover:text-amber-600 hover:bg-amber-50 dark:hover:bg-amber-900/30 dark:hover:text-amber-400 rounded-lg transition-colors"
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
            className="p-2 h-auto w-auto text-slate-400 hover:text-slate-600 hover:bg-slate-100 dark:hover:bg-slate-800 dark:hover:text-slate-300 rounded-lg transition-colors"
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
            className="p-2 h-auto w-auto text-slate-400 hover:text-red-600 hover:bg-red-50 dark:hover:bg-red-900/30 dark:hover:text-red-400 rounded-lg transition-colors"
            disabled={isDeleting}
          >
            {isDeleting ? (
              <Loader2 className="w-4 h-4 animate-spin" />
            ) : (
              <Trash2 className="w-4 h-4" />
            )}
          </Button>
        </>
      }
    />
  );
}
