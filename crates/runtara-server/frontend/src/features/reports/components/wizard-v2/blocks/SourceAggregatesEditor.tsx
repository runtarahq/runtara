// Phase 11 follow-up: surface `source.aggregates` + `source.group_by`
// in the wizard. Chart and metric blocks require at least one
// aggregate (the server rejects them otherwise), so this editor is
// mounted on those block types' inline editors. Author flow:
//
//   1. Add a Source aggregate row (op + optional field, alias auto-
//      synthesized).
//   2. Pick group-by fields if you want grouped rows (typical for
//      charts; metric blocks usually leave group_by empty for a
//      scalar).
//   3. The chart/metric editor's field pickers now resolve aggregate
//      aliases plus schema columns.
//
// The editor sets `source.mode='aggregate'` automatically when at
// least one aggregate is present, so the server's render path picks
// the right code path.

import { Schema } from '@/generated/RuntaraRuntimeApi';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { Plus, Trash2 } from 'lucide-react';
import { ReportSource } from '../../../types';
import type {
  ReportAggregateFn,
  ReportAggregateSpec,
} from '@/generated/RuntaraRuntimeApi';

const AGG_OPS: Array<{ value: ReportAggregateFn; label: string; requiresField: boolean }> = [
  { value: 'count', label: 'count', requiresField: false },
  { value: 'sum', label: 'sum', requiresField: true },
  { value: 'avg', label: 'avg', requiresField: true },
  { value: 'min', label: 'min', requiresField: true },
  { value: 'max', label: 'max', requiresField: true },
];

interface SourceAggregatesEditorProps {
  source: ReportSource;
  schemas: Schema[];
  onChange: (source: ReportSource) => void;
}

export function SourceAggregatesEditor({
  source,
  schemas,
  onChange,
}: SourceAggregatesEditorProps) {
  const aggregates = source.aggregates ?? [];
  const groupBy = source.groupBy ?? [];
  const schema = schemas.find((s) => s.name === source.schema);
  const schemaFields = schema?.columns.map((c) => c.name) ?? [];

  const commit = (next: Partial<ReportSource>) => {
    const merged: ReportSource = { ...source, ...next };
    // Auto-flip mode based on whether there's at least one aggregate.
    if ((merged.aggregates ?? []).length > 0) {
      merged.mode = 'aggregate';
    } else if (source.mode === 'aggregate') {
      merged.mode = 'filter';
    }
    onChange(merged);
  };

  const updateAggregates = (next: ReportAggregateSpec[]) =>
    commit({ aggregates: next });

  const addAggregate = () => {
    const baseAlias = `value_${aggregates.length + 1}`;
    updateAggregates([
      ...aggregates,
      { alias: baseAlias, op: 'count' as ReportAggregateFn },
    ]);
  };

  const updateGroupBy = (next: string[]) =>
    commit({ groupBy: next.length ? next : undefined });

  return (
    <div className="grid gap-3 rounded border border-dashed p-3">
      <header className="flex items-center justify-between">
        <div className="grid">
          <Label className="text-xs font-semibold">Source aggregates</Label>
          <span className="text-[10px] text-muted-foreground">
            Required for chart and metric blocks. Each row produces one
            column you can reference by alias.
          </span>
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-7"
          onClick={addAggregate}
        >
          <Plus className="mr-1 h-3 w-3" /> Add aggregate
        </Button>
      </header>

      {aggregates.length === 0 ? (
        <p
          className="text-xs text-muted-foreground"
          data-testid="source-aggregates-empty"
        >
          No aggregates yet. Add one to make this block render.
        </p>
      ) : (
        <div className="grid gap-2">
          {aggregates.map((agg, index) => {
            const opMeta = AGG_OPS.find((o) => o.value === agg.op);
            const requiresField = opMeta?.requiresField ?? false;
            return (
              <div
                key={index}
                data-testid={`source-aggregate-row-${index}`}
                className="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_minmax(0,1fr)_auto] items-center gap-2"
              >
                <Input
                  value={agg.alias}
                  placeholder="alias"
                  className="h-8 text-xs"
                  aria-label="Aggregate alias"
                  onChange={(event) =>
                    updateAggregates(
                      aggregates.map((a, i) =>
                        i === index
                          ? { ...a, alias: event.target.value }
                          : a
                      )
                    )
                  }
                />
                <Select
                  value={agg.op}
                  onValueChange={(value) =>
                    updateAggregates(
                      aggregates.map((a, i) =>
                        i === index
                          ? {
                              ...a,
                              op: value as ReportAggregateFn,
                              // Drop `field` when switching to count
                              // since count doesn't take one.
                              field:
                                AGG_OPS.find((o) => o.value === value)
                                  ?.requiresField
                                  ? a.field
                                  : null,
                            }
                          : a
                      )
                    )
                  }
                >
                  <SelectTrigger
                    aria-label="Aggregate op"
                    className="h-8 text-xs"
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {AGG_OPS.map((op) => (
                      <SelectItem key={op.value} value={op.value}>
                        {op.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <Select
                  value={agg.field ?? ''}
                  onValueChange={(value) =>
                    updateAggregates(
                      aggregates.map((a, i) =>
                        i === index ? { ...a, field: value || null } : a
                      )
                    )
                  }
                  disabled={!requiresField}
                >
                  <SelectTrigger
                    aria-label="Aggregate field"
                    className="h-8 text-xs"
                  >
                    <SelectValue
                      placeholder={
                        requiresField ? 'Pick field' : '— not used —'
                      }
                    />
                  </SelectTrigger>
                  <SelectContent>
                    {agg.field && !schemaFields.includes(agg.field) ? (
                      <SelectItem disabled value={agg.field}>
                        {agg.field}
                      </SelectItem>
                    ) : null}
                    {schemaFields.map((field) => (
                      <SelectItem key={field} value={field}>
                        {field}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="h-8 w-8 text-destructive"
                  aria-label="Remove aggregate"
                  onClick={() =>
                    updateAggregates(aggregates.filter((_, i) => i !== index))
                  }
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              </div>
            );
          })}
        </div>
      )}

      <div className="grid gap-1.5">
        <Label className="text-xs">Group by</Label>
        <div className="flex flex-wrap items-center gap-2">
          {groupBy.length === 0 ? (
            <span className="text-xs text-muted-foreground">
              Empty = a single scalar row across all matching records.
            </span>
          ) : (
            groupBy.map((field, index) => (
              <span
                key={`${field}-${index}`}
                className="inline-flex items-center gap-1 rounded border bg-background px-2 py-0.5 text-xs"
              >
                {field}
                <button
                  type="button"
                  aria-label={`Remove ${field} from group by`}
                  className="text-muted-foreground hover:text-destructive"
                  onClick={() =>
                    updateGroupBy(groupBy.filter((_, i) => i !== index))
                  }
                >
                  ×
                </button>
              </span>
            ))
          )}
          <Select
            value=""
            onValueChange={(value) => {
              if (!value) return;
              if (groupBy.includes(value)) return;
              updateGroupBy([...groupBy, value]);
            }}
          >
            <SelectTrigger
              aria-label="Add group-by field"
              className="h-7 w-[160px] text-xs"
            >
              <SelectValue placeholder="+ Add field" />
            </SelectTrigger>
            <SelectContent>
              {schemaFields
                .filter((field) => !groupBy.includes(field))
                .map((field) => (
                  <SelectItem key={field} value={field}>
                    {field}
                  </SelectItem>
                ))}
            </SelectContent>
          </Select>
        </div>
      </div>
    </div>
  );
}
