import { useState } from 'react';
import { CalendarRange, ChevronDown } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/shared/components/ui/popover';
import { cn } from '@/lib/utils';

import {
  type DateFormat,
  type DateRange,
  RANGE_PRESETS,
  describeRange,
  fromLocalInput,
  matchingPreset,
  presetRange,
  toLocalInput,
} from './date-values';

/**
 * A period: one button that shows the chosen range and opens presets and
 * From/To pickers. A preset applies at once; custom bounds are drafted and
 * applied together, so a half-edited range never runs. The value is
 * `{from, to}`.
 */
export function RangeControl({
  id,
  labelledBy,
  format,
  value,
  disabled,
  invalid,
  onChange,
}: {
  id: string;
  labelledBy?: string;
  format: DateFormat;
  value: unknown;
  disabled: boolean;
  invalid?: boolean;
  onChange: (value: DateRange) => void;
}) {
  const [open, setOpen] = useState(false);
  const range: DateRange =
    value && typeof value === 'object' && !Array.isArray(value)
      ? (value as DateRange)
      : {};
  const [draft, setDraft] = useState<DateRange>(range);
  const active = matchingPreset(range, format);
  const isInverted = (r: DateRange) =>
    !!r.from &&
    !!r.to &&
    (format === 'date' ? r.from > r.to : Date.parse(r.from) > Date.parse(r.to));
  const inverted = isInverted(range);
  const draftInverted = isInverted(draft);
  const shown = (v: string | undefined) =>
    format === 'date' ? (v ?? '') : toLocalInput(v);
  const stored = (v: string) => (format === 'date' ? v : fromLocalInput(v));
  return (
    <Popover
      open={open}
      onOpenChange={(next) => {
        if (next) setDraft(range);
        setOpen(next);
      }}
    >
      <PopoverTrigger asChild>
        <button
          id={id}
          type="button"
          disabled={disabled}
          aria-invalid={invalid || inverted || undefined}
          aria-labelledby={
            labelledBy ? `${labelledBy} ${id}-value` : `${id}-value`
          }
          className={cn(
            'flex h-8 w-full items-center gap-2 rounded-md border border-input bg-background px-3 text-left text-sm transition-colors hover:bg-muted/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50',
            (invalid || inverted) && 'border-destructive'
          )}
        >
          <CalendarRange className="size-4 shrink-0 text-muted-foreground" />
          <span id={`${id}-value`} className="min-w-0 flex-1 truncate">
            {describeRange(range, format)}
          </span>
          <ChevronDown className="size-4 shrink-0 text-muted-foreground" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        className="flex w-auto flex-col gap-3 p-3 sm:flex-row"
        aria-label="Choose a period"
      >
        <ul className="flex flex-col gap-0.5 sm:w-32" aria-label="Presets">
          {RANGE_PRESETS.map((preset) => (
            <li key={preset.label}>
              <button
                type="button"
                aria-pressed={preset === active}
                className={cn(
                  'w-full rounded px-2 py-1 text-left text-sm hover:bg-muted',
                  preset === active && 'bg-primary/10 font-medium text-primary'
                )}
                onClick={() => {
                  onChange(presetRange(preset, format));
                  setOpen(false);
                }}
              >
                {preset.label}
              </button>
            </li>
          ))}
        </ul>
        <div className="space-y-2 sm:border-l sm:pl-3">
          {(['from', 'to'] as const).map((bound) => (
            <label key={bound} className="block space-y-1 text-sm">
              <span className="font-medium">
                {bound === 'from' ? 'From' : 'To'}
              </span>
              <Input
                type={format === 'date' ? 'date' : 'datetime-local'}
                value={shown(draft[bound])}
                onChange={(event) =>
                  setDraft({ ...draft, [bound]: stored(event.target.value) })
                }
              />
            </label>
          ))}
          {draftInverted && (
            <p role="alert" className="text-xs text-destructive">
              From is after To.
            </p>
          )}
          <Button
            size="sm"
            className="w-full"
            disabled={!draft.from || !draft.to || draftInverted}
            onClick={() => {
              onChange({ from: draft.from, to: draft.to });
              setOpen(false);
            }}
          >
            Apply
          </Button>
        </div>
      </PopoverContent>
    </Popover>
  );
}
