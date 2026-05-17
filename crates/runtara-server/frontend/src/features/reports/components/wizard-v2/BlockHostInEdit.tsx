// In-place block rendering for the wizard's edit mode. Wraps the
// existing viewer `ReportBlockHost` and overlays edit-chrome: title
// label, hover-revealed action buttons (configure, delete, drag handle).

import { Button } from '@/shared/components/ui/button';
import { Pencil, Trash2 } from 'lucide-react';
import { ReportBlockDefinition, ReportBlockResult } from '../../types';
import { ReportBlockHost } from '../ReportBlockHost';

interface BlockHostInEditProps {
  block: ReportBlockDefinition;
  blockResult?: ReportBlockResult;
  reportId?: string;
  filters: Record<string, unknown>;
  onConfigure: () => void;
  onDelete: () => void;
}

/** Renders the block exactly as the viewer would, plus hover-revealed
 *  edit affordances. The wrapping `<div>` is the focusable surface for
 *  hover state; pointer-events on the rendered block remain interactive
 *  so the existing block widgets keep responding. */
export function BlockHostInEdit({
  block,
  blockResult,
  reportId,
  filters,
  onConfigure,
  onDelete,
}: BlockHostInEditProps) {
  return (
    <div className="group/wizard-block relative rounded-md border bg-card p-2 transition-shadow hover:shadow-sm">
      <div className="mb-2 flex items-center justify-between gap-2">
        <div className="min-w-0">
          <p className="truncate text-xs font-medium text-muted-foreground">
            {block.title || block.id}
          </p>
          <p className="text-[10px] uppercase tracking-wider text-muted-foreground/70">
            {block.type}
          </p>
        </div>
        <div className="flex items-center gap-1 opacity-0 transition-opacity group-hover/wizard-block:opacity-100 focus-within:opacity-100">
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-7 w-7"
            title="Configure block"
            onClick={onConfigure}
          >
            <Pencil className="h-3.5 w-3.5" />
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-7 w-7 text-destructive"
            title="Remove block"
            onClick={onDelete}
          >
            <Trash2 className="h-3.5 w-3.5" />
          </Button>
        </div>
      </div>
      <div className="pointer-events-auto">
        {reportId ? (
          <ReportBlockHost
            block={block}
            reportId={reportId}
            initialResult={blockResult}
            filters={filters}
            className="my-0"
          />
        ) : (
          // Preview not available yet (no report id; e.g. first-save
          // path before the report is created).
          <p className="rounded border border-dashed bg-muted/30 px-2 py-3 text-xs text-muted-foreground">
            Preview becomes available after saving the report. Configure the
            block to set its data source.
          </p>
        )}
      </div>
    </div>
  );
}
