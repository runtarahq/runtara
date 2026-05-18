// Phase 11: in-place block editor. Replaces the cell's preview with the
// full <BlockEditor> form inside the grid slot. Escape + Done dismiss
// back to preview mode. Form content scrolls internally when it would
// otherwise stretch the entire grid (max-h 60vh cap).

import { Schema } from '@/generated/RuntaraRuntimeApi';
import { Button } from '@/shared/components/ui/button';
import { Check, GripVertical, Trash2 } from 'lucide-react';
import { useEffect } from 'react';
import { ReportBlockDefinition, ReportDatasetDefinition } from '../../types';
import { BlockEditor } from './blocks/BlockEditor';

interface InlineBlockEditorProps {
  block: ReportBlockDefinition;
  schemas: Schema[];
  datasets: ReportDatasetDefinition[];
  /** Forwarded dnd-kit `useSortable` attributes + listeners. */
  dragHandleProps?: Record<string, unknown>;
  /** Save+close. The block has already been persisted via onChange on
   *  every keystroke — this is a "done editing" affordance, not a
   *  transactional save. */
  onDone: () => void;
  onChange: (next: ReportBlockDefinition) => void;
  onDelete: () => void;
}

export function InlineBlockEditor({
  block,
  schemas,
  datasets,
  dragHandleProps,
  onDone,
  onChange,
  onDelete,
}: InlineBlockEditorProps) {
  // Escape closes the inline editor. We listen on document so the
  // handler fires regardless of which form input has focus.
  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.stopPropagation();
        onDone();
      }
    };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [onDone]);

  return (
    <div
      className="rounded-md border bg-card shadow-sm"
      data-block-id={block.id}
      data-testid={`inline-editor-${block.id}`}
    >
      <header className="flex items-center justify-between gap-2 border-b bg-muted/30 px-3 py-2">
        <div className="flex min-w-0 items-center gap-2">
          {dragHandleProps ? (
            <button
              type="button"
              className="cursor-grab rounded p-0.5 text-muted-foreground hover:bg-muted active:cursor-grabbing"
              title="Drag to reorder"
              aria-label="Drag block"
              {...dragHandleProps}
            >
              <GripVertical className="h-3.5 w-3.5" />
            </button>
          ) : null}
          <div className="min-w-0">
            <p className="truncate text-xs font-medium text-foreground">
              Editing {block.title || block.id}
            </p>
            <p className="text-[10px] uppercase tracking-wider text-muted-foreground/70">
              {block.type}
            </p>
          </div>
        </div>
        <div className="flex items-center gap-1">
          <Button
            type="button"
            variant="default"
            size="sm"
            className="h-7 px-2"
            onClick={onDone}
            title="Stop editing (Esc)"
          >
            <Check className="mr-1 h-3 w-3" /> Done
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-7 w-7 text-destructive"
            title="Remove block"
            aria-label="Remove block"
            onClick={onDelete}
          >
            <Trash2 className="h-3.5 w-3.5" />
          </Button>
        </div>
      </header>
      <div className="max-h-[60vh] overflow-y-auto p-3">
        <BlockEditor
          block={block}
          schemas={schemas}
          datasets={datasets}
          onChange={onChange}
        />
      </div>
    </div>
  );
}
