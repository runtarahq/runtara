import { useEffect, useState } from 'react';
import type { Edge } from '@xyflow/react';
import { Settings2, Trash2 } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import { FieldError } from '@/shared/components/ui/form';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/shared/components/ui/popover';
import { Textarea } from '@/shared/components/ui/textarea';
import { ConditionEditor } from '@/shared/components/ui/condition-editor';
import type { ConditionSuggestion } from '@/shared/components/ui/condition-editor';
import { cn } from '@/lib/utils.ts';

export function getRouteCondition(edge: Edge): unknown {
  return (edge.data as { condition?: unknown } | undefined)?.condition;
}

export function getRoutePriority(edge: Edge): number | undefined {
  const priority = (edge.data as { priority?: unknown } | undefined)?.priority;
  return typeof priority === 'number' && Number.isFinite(priority)
    ? priority
    : undefined;
}

function formatRouteCondition(condition: unknown): string {
  if (condition === undefined) return '';
  return JSON.stringify(condition, null, 2);
}

function parseRouteCondition(
  value: string
): { ok: true; condition: unknown } | { ok: false; message: string } {
  if (!value.trim()) {
    return { ok: true, condition: undefined };
  }

  try {
    return { ok: true, condition: JSON.parse(value) };
  } catch (error) {
    return {
      ok: false,
      message: error instanceof Error ? error.message : 'Invalid JSON',
    };
  }
}

function parseRoutePriority(
  value: string
): { ok: true; priority: number | undefined } | { ok: false; message: string } {
  if (!value.trim()) {
    return { ok: true, priority: undefined };
  }

  const priority = Number(value);
  if (!Number.isFinite(priority)) {
    return { ok: false, message: 'Priority must be a number.' };
  }

  return { ok: true, priority };
}

/**
 * Whether the visual ConditionEditor can represent this condition. It needs an
 * operation object (`op` + `arguments`); anything else (or nothing) stays in
 * the Advanced JSON editor. Empty conditions are visually editable — the
 * builder starts from scratch.
 */
function isVisuallyEditableRouteCondition(condition: unknown): boolean {
  if (condition === undefined) return true;
  return (
    typeof condition === 'object' &&
    condition !== null &&
    'op' in condition &&
    (condition as { op?: unknown }).op !== undefined &&
    'arguments' in condition
  );
}

/**
 * Condition and priority of one route (edge), edited from the canvas: the
 * cog beside a selected edge's "+" opens it.
 */
export function RouteSettingsPopover({
  edge,
  sourceName,
  targetName,
  conditionSuggestions,
  onUpdate,
  onDelete,
}: {
  edge: Edge;
  sourceName: string;
  targetName: string;
  conditionSuggestions: ConditionSuggestion[];
  onUpdate: (updates: { condition?: unknown; priority?: number }) => void;
  onDelete: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [conditionText, setConditionText] = useState('');
  const [conditionMode, setConditionMode] = useState<'visual' | 'json'>(
    'visual'
  );
  const [conditionResetKey, setConditionResetKey] = useState(0);
  const [priorityText, setPriorityText] = useState('');
  const [error, setError] = useState<string | null>(null);
  const hasMetadata =
    getRouteCondition(edge) !== undefined ||
    getRoutePriority(edge) !== undefined;

  useEffect(() => {
    const condition = getRouteCondition(edge);
    setConditionText(formatRouteCondition(condition));
    setConditionMode(
      isVisuallyEditableRouteCondition(condition) ? 'visual' : 'json'
    );
    setConditionResetKey((key) => key + 1);
    const priority = getRoutePriority(edge);
    setPriorityText(priority !== undefined ? String(priority) : '');
    setError(null);
    // Reset from the edge whenever it (or its data) changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [edge.id, edge.data]);

  const handleApply = () => {
    const conditionResult = parseRouteCondition(conditionText);
    if (!conditionResult.ok) {
      setError(`Condition JSON: ${conditionResult.message}`);
      return;
    }

    const priorityResult = parseRoutePriority(priorityText);
    if (!priorityResult.ok) {
      setError(priorityResult.message);
      return;
    }

    onUpdate({
      condition: conditionResult.condition,
      priority: priorityResult.priority,
    });
    setError(null);
    setOpen(false);
  };

  const handleClear = () => {
    setConditionText('');
    setPriorityText('');
    setError(null);
    setConditionResetKey((key) => key + 1);
    onUpdate({ condition: undefined, priority: undefined });
  };

  const handleToggleConditionMode = () => {
    if (conditionMode === 'visual') {
      setConditionMode('json');
      return;
    }

    const conditionResult = parseRouteCondition(conditionText);
    if (!conditionResult.ok) {
      setError(`Condition JSON: ${conditionResult.message}`);
      return;
    }
    if (!isVisuallyEditableRouteCondition(conditionResult.condition)) {
      setError('This condition shape is only editable as JSON.');
      return;
    }

    setError(null);
    setConditionResetKey((key) => key + 1);
    setConditionMode('visual');
  };

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          type="button"
          variant="secondary"
          bordered
          size="icon"
          className={cn(
            'size-5 rounded-full shadow-sm transition-all hover:shadow-md [&_svg]:size-3',
            hasMetadata && 'text-primary-text'
          )}
          aria-label={`Edit route condition and priority from ${sourceName} to ${targetName}`}
          title="Route condition and priority"
          data-testid="edge-route-settings"
          onClick={(event) => event.stopPropagation()}
        >
          <Settings2 />
        </Button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        side="right"
        className="nodrag nopan w-[26rem] space-y-4"
        onOpenAutoFocus={(event) => event.preventDefault()}
      >
        <div>
          <p className="text-sm font-semibold text-foreground">
            Route settings
          </p>
          <p className="text-xs text-muted-foreground">
            From {sourceName} to {targetName}
          </p>
        </div>

        <div className="space-y-2">
          <Label htmlFor={`route-priority-${edge.id}`}>Priority</Label>
          <Input
            id={`route-priority-${edge.id}`}
            type="number"
            inputMode="numeric"
            value={priorityText}
            onChange={(event) => setPriorityText(event.target.value)}
            placeholder="Optional"
          />
        </div>

        <div className="space-y-2">
          <div className="flex items-center justify-between">
            <Label htmlFor={`route-condition-${edge.id}`}>Condition</Label>
            <Button
              type="button"
              variant="secondary"
              size="sm"
              className="h-6 px-2 text-xs"
              onClick={handleToggleConditionMode}
              data-testid="edge-route-condition-mode"
            >
              {conditionMode === 'visual' ? 'Advanced (JSON)' : 'Visual editor'}
            </Button>
          </div>
          {conditionMode === 'visual' ? (
            <ConditionEditor
              key={`${edge.id}-${conditionResetKey}`}
              value={conditionText || undefined}
              onChange={setConditionText}
              suggestions={conditionSuggestions}
            />
          ) : (
            <Textarea
              id={`route-condition-${edge.id}`}
              value={conditionText}
              onChange={(event) => setConditionText(event.target.value)}
              placeholder='{"type": "operation", "op": "EQ", "arguments": ["data.status", "ready"]}'
              className="min-h-32 font-mono text-xs"
            />
          )}
          {error && <FieldError>{error}</FieldError>}
        </div>

        <div className="flex items-center justify-between gap-2">
          <Button
            type="button"
            variant="secondaryDestructive"
            size="sm"
            className="gap-1.5"
            onClick={() => {
              onDelete();
              setOpen(false);
            }}
            data-testid="edge-route-delete"
          >
            <Trash2 className="size-3.5" aria-hidden="true" />
            Delete route
          </Button>
          <div className="flex gap-2">
            <Button
              type="button"
              variant="secondary"
              size="sm"
              onClick={handleClear}
            >
              Clear
            </Button>
            <Button type="button" size="sm" onClick={handleApply}>
              Apply
            </Button>
          </div>
        </div>
      </PopoverContent>
    </Popover>
  );
}
