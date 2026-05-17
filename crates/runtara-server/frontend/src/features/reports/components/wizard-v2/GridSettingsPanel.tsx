// Single per-grid settings panel. Covers title, description, column
// count, column widths, and visibility. Per-grid-item colSpan / rowSpan
// editing happens on the block editor (the item wraps the block, so
// it's a property of how the block sits in its parent grid).

import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import { Button } from '@/shared/components/ui/button';
import { Plus, Trash2 } from 'lucide-react';
import { ReportGridLayoutNode } from '../../types';

interface GridSettingsPanelProps {
  grid: ReportGridLayoutNode & { type: 'grid' };
  onChange: (
    updater: (grid: ReportGridLayoutNode) => ReportGridLayoutNode
  ) => void;
}

export function GridSettingsPanel({ grid, onChange }: GridSettingsPanelProps) {
  const columns = grid.columns ?? 1;
  const widths = grid.columnWidths;

  const setColumns = (value: number) => {
    onChange((g) => ({
      ...g,
      columns: Math.max(1, Math.min(12, value)),
      // Drop columnWidths when the count changes; they'd no longer align.
      columnWidths: undefined,
    }));
  };

  const setColumnWidth = (index: number, value: number) => {
    onChange((g) => {
      const current = g.columnWidths ?? Array.from({ length: columns }, () => 1);
      const next = [...current];
      while (next.length < columns) next.push(1);
      next[index] = Math.max(0.01, value);
      return { ...g, columnWidths: next.slice(0, columns) };
    });
  };

  const clearColumnWidths = () => {
    onChange((g) => {
      const cleaned = { ...g };
      delete (cleaned as { columnWidths?: number[] }).columnWidths;
      return cleaned;
    });
  };

  return (
    <div className="grid gap-2">
      <div className="grid grid-cols-2 gap-2">
        <div className="grid gap-1">
          <Label className="text-xs">Title</Label>
          <Input
            value={grid.title ?? ''}
            className="h-8 text-xs"
            onChange={(event) =>
              onChange((g) => ({
                ...g,
                title: event.target.value || undefined,
              }))
            }
          />
        </div>
        <div className="grid gap-1">
          <Label className="text-xs">Columns</Label>
          <Input
            type="number"
            min={1}
            max={12}
            value={columns}
            className="h-8 text-xs"
            onChange={(event) => setColumns(parseInt(event.target.value, 10))}
          />
        </div>
      </div>
      <div className="grid gap-1">
        <Label className="text-xs">Description</Label>
        <Input
          value={grid.description ?? ''}
          className="h-8 text-xs"
          onChange={(event) =>
            onChange((g) => ({
              ...g,
              description: event.target.value || undefined,
            }))
          }
        />
      </div>
      <div className="grid gap-1.5 rounded border p-2">
        <div className="flex items-center justify-between">
          <Label className="text-xs">Column widths (fractions)</Label>
          {widths && widths.length > 0 ? (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-6"
              onClick={clearColumnWidths}
            >
              <Trash2 className="mr-1 h-3 w-3" /> Equal split
            </Button>
          ) : (
            <Button
              type="button"
              variant="ghost"
              size="sm"
              className="h-6"
              onClick={() =>
                onChange((g) => ({
                  ...g,
                  columnWidths: Array.from({ length: columns }, () => 1),
                }))
              }
            >
              <Plus className="mr-1 h-3 w-3" /> Customize
            </Button>
          )}
        </div>
        {widths && widths.length > 0 ? (
          <div className="grid grid-cols-4 gap-2">
            {Array.from({ length: columns }).map((_, i) => (
              <Input
                key={i}
                type="number"
                min={0.01}
                step={0.1}
                value={widths[i] ?? 1}
                className="h-8 text-xs"
                onChange={(event) =>
                  setColumnWidth(i, parseFloat(event.target.value))
                }
              />
            ))}
          </div>
        ) : (
          <p className="text-xs text-muted-foreground">
            Columns share equal width. Customize to pin fractional widths
            (e.g. 2 fr / 1 fr for a wide-left layout).
          </p>
        )}
      </div>
    </div>
  );
}
