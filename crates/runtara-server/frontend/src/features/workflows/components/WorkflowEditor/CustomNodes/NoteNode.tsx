import { memo, useCallback, useState, useRef, useEffect } from 'react';
import { NodeProps, NodeResizer } from '@xyflow/react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { GripHorizontal, Link2, X } from 'lucide-react';
import { cn } from '@/lib/utils.ts';
import {
  snapToGrid,
  SNAP_GRID_SIZE,
} from '@/features/workflows/config/workflow-editor.ts';
import { useWorkflowStore } from '@/features/workflows/stores/workflowStore.ts';
import { useExecutionStore } from '@/features/workflows/stores/executionStore';
import { Textarea } from '@/shared/components/ui/textarea.tsx';

const handleStyle = {
  width: '8px',
  height: '8px',
  borderRadius: '2px',
  backgroundColor: '#fbbf24', // yellow-400
  border: '1px solid #f59e0b', // yellow-500
};

const NoteNodeComponent = ({ id, data, selected, dragging }: NodeProps) => {
  // A note just added from the toolbar opens straight into its editor.
  const [isEditing, setIsEditing] = useState(data.autoEdit === true);
  const anchorStepName =
    typeof data.anchorStepName === 'string' ? data.anchorStepName : null;
  const [content, setContent] = useState<string>(
    typeof data.content === 'string' ? data.content : ''
  );
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const removeNode = useWorkflowStore((state) => state.removeNode);
  const updateNote = useWorkflowStore((state) => state.updateNote);

  // Check if workflow is executing (read-only mode)
  const isExecuting = useExecutionStore((state) => !!state.executingInstanceId);

  // Sync content from store when data changes
  useEffect(() => {
    setContent(typeof data.content === 'string' ? data.content : '');
  }, [data.content]);

  // Focus textarea when entering edit mode
  useEffect(() => {
    if (isEditing && textareaRef.current) {
      textareaRef.current.focus();
      // Move cursor to end
      const length = textareaRef.current.value.length;
      textareaRef.current.setSelectionRange(length, length);
    }
  }, [isEditing]);

  // Exit edit mode when dragging starts
  useEffect(() => {
    if (dragging && isEditing) {
      setIsEditing(false);
    }
  }, [dragging, isEditing]);

  const handleResize = useCallback(
    (_event: unknown, params: { width: number; height: number }) => {
      // Always snap to grid when resizing for consistency
      const width = snapToGrid(params.width);
      const height = snapToGrid(params.height);
      updateNote(id, { width, height }, false);
      return { width, height };
    },
    [id, updateNote]
  );

  const handleResizeEnd = useCallback(
    (_event: unknown, params: { width: number; height: number }) => {
      updateNote(id, {
        width: snapToGrid(params.width),
        height: snapToGrid(params.height),
      });
    },
    [id, updateNote]
  );

  const handleDelete = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      removeNode(id);
    },
    [id, removeNode]
  );

  const handleDoubleClick = useCallback(() => {
    // Don't allow editing during execution
    if (!isEditing && !isExecuting) {
      setIsEditing(true);
    }
  }, [isEditing, isExecuting]);

  const handleBlur = useCallback(() => {
    setIsEditing(false);
    // Save the text, and drop the one-shot autoEdit flag so the note does
    // not reopen on the next render.
    const currentContent = typeof data.content === 'string' ? data.content : '';
    if (content !== currentContent || data.autoEdit) {
      updateNote(id, { content });
    }
  }, [content, data.autoEdit, data.content, id, updateNote]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      // Exit edit mode on Escape
      if (e.key === 'Escape') {
        e.preventDefault();
        setIsEditing(false);
        setContent(typeof data.content === 'string' ? data.content : ''); // Reset to original content
      }
      // Don't exit on Enter - allow multiline editing
    },
    [data.content]
  );

  return (
    <div
      className={cn(
        'group relative h-full w-full rounded-md',
        'bg-yellow-50 dark:bg-yellow-950/20',
        'border-2',
        selected
          ? 'border-yellow-500 shadow-lg dark:border-yellow-600'
          : 'border-yellow-300 shadow-md dark:border-yellow-700',
        'transition-all duration-200',
        'hover:shadow-lg',
        isEditing && 'nodrag nopan [&_*]:select-text'
      )}
      style={{ userSelect: isEditing ? 'text' : undefined }}
      onDoubleClick={handleDoubleClick}
    >
      <NodeResizer
        handleStyle={handleStyle}
        onResize={handleResize}
        onResizeEnd={handleResizeEnd}
        minWidth={SNAP_GRID_SIZE * 16} // 192px minimum
        minHeight={SNAP_GRID_SIZE * 8} // 96px minimum
        isVisible={selected && !isExecuting}
      />

      {/* Header: the drag handle (React Flow `dragHandle`), what the note is
          attached to, and delete. Shown on hover and while selected so a
          resting note reads as just its text. */}
      {!isExecuting && (
        <div
          className={cn(
            'note-drag-handle absolute inset-x-0 top-0 z-10 flex h-6 items-center gap-1.5 rounded-t-[4px] px-1.5',
            'cursor-grab active:cursor-grabbing',
            'bg-yellow-100/90 text-yellow-700 dark:bg-yellow-900/60 dark:text-yellow-300',
            'opacity-0 transition-opacity duration-150 group-hover:opacity-100',
            (selected || dragging) && 'opacity-100'
          )}
          title="Drag to move. Drop next to a step to attach the note to it."
        >
          <GripHorizontal className="size-3.5 shrink-0" aria-hidden="true" />
          <span className="flex min-w-0 flex-1 items-center gap-1 truncate text-2xs">
            {anchorStepName ? (
              <>
                <Link2 className="size-3 shrink-0" aria-hidden="true" />
                <span className="truncate">{anchorStepName}</span>
              </>
            ) : (
              'Note'
            )}
          </span>
          <button
            type="button"
            onClick={handleDelete}
            className={cn(
              'nodrag shrink-0 rounded-sm p-0.5',
              'hover:bg-yellow-200 dark:hover:bg-yellow-800/60',
              'transition-colors duration-150'
            )}
            aria-label="Delete note"
            title="Delete note"
          >
            <X className="size-3.5" />
          </button>
        </div>
      )}

      {/* Content area */}
      <div className="h-full w-full overflow-hidden rounded-md px-3 pb-3 pt-7 text-xs leading-snug">
        {isEditing ? (
          <Textarea
            ref={textareaRef}
            value={content}
            onChange={(e) => setContent(e.target.value)}
            onBlur={handleBlur}
            onKeyDown={handleKeyDown}
            className={cn(
              'h-full w-full resize-none',
              'border-none bg-transparent',
              'text-yellow-900 dark:text-yellow-100',
              'placeholder:text-yellow-500 dark:placeholder:text-yellow-600',
              'focus:outline-none focus-visible:ring-0',
              'text-xs leading-snug',
              'select-text'
            )}
            placeholder="Type your note... (Markdown supported)"
          />
        ) : (
          <div
            className={cn(
              'text-yellow-900 dark:text-yellow-100',
              'cursor-text select-text',
              // Custom markdown styling
              '[&>h1]:mb-1 [&>h1]:text-sm [&>h1]:font-bold',
              '[&>h2]:mb-1 [&>h2]:text-[13px] [&>h2]:font-bold',
              '[&>h3]:mb-1 [&>h3]:text-xs [&>h3]:font-semibold',
              '[&>p:last-child]:mb-0 [&>p]:mb-1',
              '[&>ul]:mb-1 [&>ul]:ml-3 [&>ul]:list-disc',
              '[&>ol]:mb-1 [&>ol]:ml-3 [&>ol]:list-decimal',
              '[&>ol>li]:mb-0.5 [&>ul>li]:mb-0.5',
              '[&>code]:rounded [&>code]:bg-yellow-200/50 [&>code]:px-1 [&>code]:dark:bg-yellow-900/50',
              '[&>pre]:mb-1 [&>pre]:overflow-x-auto [&>pre]:rounded [&>pre]:bg-yellow-200/50 [&>pre]:p-2 [&>pre]:dark:bg-yellow-900/50',
              '[&>blockquote]:mb-1 [&>blockquote]:border-l-2 [&>blockquote]:border-yellow-400 [&>blockquote]:pl-2 [&>blockquote]:italic',
              '[&_strong]:font-bold',
              '[&_em]:italic',
              '[&_a]:text-yellow-700 [&_a]:underline [&_a]:dark:text-yellow-400',
              // Table styling
              '[&_table]:mb-1 [&_table]:w-full [&_table]:border-collapse',
              '[&_th]:border [&_th]:border-yellow-400 [&_th]:bg-yellow-200/50 [&_th]:px-1.5 [&_th]:py-0.5 [&_th]:text-left [&_th]:font-semibold [&_th]:dark:border-yellow-600 [&_th]:dark:bg-yellow-900/50',
              '[&_td]:border [&_td]:border-yellow-300 [&_td]:px-1.5 [&_td]:py-0.5 [&_td]:dark:border-yellow-700'
            )}
          >
            {content ? (
              <ReactMarkdown remarkPlugins={[remarkGfm]}>
                {content}
              </ReactMarkdown>
            ) : (
              <p className="text-xs italic text-yellow-600 dark:text-yellow-500">
                Double-click to write a note
              </p>
            )}
          </div>
        )}
      </div>
    </div>
  );
};

export const NoteNode = memo(NoteNodeComponent);
