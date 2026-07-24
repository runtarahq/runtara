import { memo, useCallback, useState, useRef, useEffect } from 'react';
import { NodeProps, NodeResizer } from '@xyflow/react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { X } from 'lucide-react';
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
  const [isEditing, setIsEditing] = useState(false);
  const [content, setContent] = useState<string>(
    typeof data.content === 'string' ? data.content : ''
  );
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const removeNode = useWorkflowStore((state) => state.removeNode);

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
    (_event: any, params: { width: number; height: number }) => {
      // Always snap to grid when resizing for consistency
      const snappedWidth = snapToGrid(params.width);
      const snappedHeight = snapToGrid(params.height);

      // Update node dimensions directly in the store
      const state = useWorkflowStore.getState();
      const updatedNodes = state.nodes.map((n) =>
        n.id === id
          ? {
              ...n,
              width: snappedWidth,
              height: snappedHeight,
              style: {
                ...n.style,
                width: snappedWidth,
                height: snappedHeight,
              },
            }
          : n
      );
      state.syncFromReactFlow(updatedNodes, state.edges);

      return { width: snappedWidth, height: snappedHeight };
    },
    [id]
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
    // Save content to store by updating the nodes array
    const currentContent = typeof data.content === 'string' ? data.content : '';
    if (content !== currentContent) {
      const state = useWorkflowStore.getState();
      const updatedNodes = state.nodes.map((n) =>
        n.id === id ? { ...n, data: { ...n.data, content } } : n
      );
      state.syncFromReactFlow(updatedNodes, state.edges);
    }
  }, [content, data.content, id]);

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
        minWidth={SNAP_GRID_SIZE * 16} // 192px minimum
        minHeight={SNAP_GRID_SIZE * 8} // 96px minimum
        isVisible={selected && !isExecuting}
      />

      {/* Delete button - hidden during execution */}
      {!isExecuting && (
        <button
          onClick={handleDelete}
          className={cn(
            'absolute right-2 top-2 z-10',
            'rounded-sm p-1',
            'bg-yellow-100 dark:bg-yellow-900/40',
            'hover:bg-yellow-200 dark:hover:bg-yellow-800/60',
            'text-yellow-700 dark:text-yellow-300',
            'transition-colors duration-200',
            'opacity-0 group-hover:opacity-100',
            selected && 'opacity-100'
          )}
          title="Delete note"
        >
          <X className="h-4 w-4" />
        </button>
      )}

      {/* Content area */}
      <div className="h-full w-full overflow-hidden rounded-md p-3 text-3xs leading-snug">
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
              'focus:outline-none focus:ring-0',
              'text-3xs leading-snug',
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
              '[&>h1]:mb-1 [&>h1]:text-[12px] [&>h1]:font-bold',
              '[&>h2]:mb-1 [&>h2]:text-2xs [&>h2]:font-bold',
              '[&>h3]:mb-1 [&>h3]:text-3xs [&>h3]:font-semibold',
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
              <p className="text-3xs italic text-yellow-500 dark:text-yellow-600">
                Click to add note...
              </p>
            )}
          </div>
        )}
      </div>
    </div>
  );
};

export const NoteNode = memo(NoteNodeComponent);
