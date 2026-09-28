import type { FormField } from './types';

/**
 * Date and date-time values as forms store them: `date` fields hold
 * `YYYY-MM-DD`, date-time fields (`format` `date-time` or the DSL's
 * `datetime`) hold an RFC 3339 UTC timestamp. Pickers show and edit them in
 * the viewer's local time.
 */
export type DateFormat = 'date' | 'date-time';

export interface DateRange {
  from?: string;
  to?: string;
}

const pad = (n: number) => String(n).padStart(2, '0');

function localDate(date: Date): string {
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
}

/** A stored date-time as a `datetime-local` input value, or '' if invalid. */
export function toLocalInput(value: unknown): string {
  if (typeof value !== 'string' || value === '') return '';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return '';
  return `${localDate(date)}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

/** A `datetime-local` input value as a stored UTC timestamp. */
export function fromLocalInput(value: string): string {
  if (value === '') return '';
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? '' : date.toISOString();
}

function dateFormat(field: FormField | undefined): DateFormat | undefined {
  if (field?.type !== 'string') return undefined;
  if (field.format === 'date') return 'date';
  if (field.format === 'date-time' || field.format === 'datetime')
    return 'date-time';
  return undefined;
}

/**
 * The format of a range field: an object whose only properties are `from`
 * and `to`, both dates or both date-times.
 */
export function rangeFormat(field: FormField): DateFormat | undefined {
  if (field.type !== 'object' || !field.properties) return undefined;
  const keys = Object.keys(field.properties).sort();
  if (keys.length !== 2 || keys[0] !== 'from' || keys[1] !== 'to') {
    return undefined;
  }
  const from = dateFormat(field.properties.from);
  return from && from === dateFormat(field.properties.to) ? from : undefined;
}

export interface RangePreset {
  label: string;
  /** First and last day included, as local midnights. */
  days: (today: Date) => [Date, Date];
}

const day = (y: number, m: number, d: number) => new Date(y, m, d);

export const RANGE_PRESETS: RangePreset[] = [
  { label: 'Today', days: (t) => [t, t] },
  {
    label: 'Last 7 days',
    days: (t) => [day(t.getFullYear(), t.getMonth(), t.getDate() - 6), t],
  },
  {
    label: 'Last 30 days',
    days: (t) => [day(t.getFullYear(), t.getMonth(), t.getDate() - 29), t],
  },
  {
    label: 'This month',
    days: (t) => [day(t.getFullYear(), t.getMonth(), 1), t],
  },
  {
    label: 'Last month',
    days: (t) => [
      day(t.getFullYear(), t.getMonth() - 1, 1),
      day(t.getFullYear(), t.getMonth(), 0),
    ],
  },
  {
    label: 'This year',
    days: (t) => [day(t.getFullYear(), 0, 1), t],
  },
];

/**
 * A preset's range in a field's format. Date ranges include both days;
 * date-time ranges run from the first day's start to the next day's start
 * after the last.
 */
export function presetRange(
  preset: RangePreset,
  format: DateFormat,
  now = new Date()
): Required<DateRange> {
  const today = day(now.getFullYear(), now.getMonth(), now.getDate());
  const [first, last] = preset.days(today);
  if (format === 'date') return { from: localDate(first), to: localDate(last) };
  const end = day(last.getFullYear(), last.getMonth(), last.getDate() + 1);
  return { from: first.toISOString(), to: end.toISOString() };
}

/** The preset a range matches today, if any. */
export function matchingPreset(
  value: DateRange,
  format: DateFormat,
  now = new Date()
): RangePreset | undefined {
  return RANGE_PRESETS.find((preset) => {
    const range = presetRange(preset, format, now);
    return same(range.from, value.from) && same(range.to, value.to);
  });
}

function same(a: string, b: string | undefined): boolean {
  if (!b) return false;
  if (a === b) return true;
  const [x, y] = [Date.parse(a), Date.parse(b)];
  return !Number.isNaN(x) && x === y;
}

/** A short description of a range for its selector's button. */
export function describeRange(
  value: DateRange,
  format: DateFormat,
  now = new Date()
): string {
  const preset = matchingPreset(value, format, now);
  if (preset) return preset.label;
  if (!value.from && !value.to) return 'Choose a period';
  const show = (v: string | undefined) => {
    if (!v) return '…';
    const date = format === 'date' ? new Date(`${v}T00:00`) : new Date(v);
    if (Number.isNaN(date.getTime())) return v;
    return format === 'date'
      ? date.toLocaleDateString(undefined, { dateStyle: 'medium' })
      : date.toLocaleString(undefined, {
          dateStyle: 'medium',
          timeStyle: 'short',
        });
  };
  return `${show(value.from)} – ${show(value.to)}`;
}
