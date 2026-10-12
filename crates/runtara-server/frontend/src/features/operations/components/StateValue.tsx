import { formatDistanceToNowStrict } from 'date-fns';

export interface StateField {
  type?: string;
  label?: string;
  description?: string;
  format?: string;
  order?: number;
  enum?: unknown[];
}

/** Presentation only. The stored value remains the query/sort value. */
export interface DisplayFormat {
  kind?: 'text' | 'number' | 'date' | 'datetime' | 'relative' | null;
  decimals?: number | null;
  prefix?: string | null;
  suffix?: string | null;
}

/** Date and minute, with the year only when it is not the current one. */
function shortDateTime(date: Date) {
  return date.toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    year:
      date.getFullYear() === new Date().getFullYear() ? undefined : 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  });
}

/** A due date and how far off it is; a past one reads as overdue. */
export function DueValue({ value }: { value: unknown }) {
  const date = typeof value === 'string' ? new Date(value) : undefined;
  if (!date || Number.isNaN(date.getTime()))
    return <StateValue value={value} />;
  const distance = formatDistanceToNowStrict(date);
  const overdue = date.getTime() < Date.now();
  return (
    <span className="block">
      <time
        dateTime={value as string}
        title={value as string}
        className="whitespace-nowrap"
      >
        {shortDateTime(date)}
      </time>
      <span
        className={`mt-0.5 block text-xs ${overdue ? 'font-medium text-warning' : 'text-muted-foreground'}`}
      >
        {overdue ? `${distance} overdue` : `in ${distance}`}
      </span>
    </span>
  );
}

export function StateValue({
  value,
  field,
  display,
}: {
  value: unknown;
  field?: StateField;
  display?: DisplayFormat;
}) {
  if (value === undefined || value === null) {
    return (
      <span className="text-muted-foreground" aria-label="No value">
        —
      </span>
    );
  }
  const kind = display?.kind ?? field?.format;
  if (
    typeof value === 'string' &&
    ['date', 'datetime', 'relative'].includes(kind ?? '')
  ) {
    const date = new Date(value);
    if (!Number.isNaN(date.getTime())) {
      const label =
        kind === 'relative'
          ? formatDistanceToNowStrict(date, { addSuffix: true })
          : kind === 'date'
            ? date.toLocaleDateString()
            : shortDateTime(date);
      return (
        <time dateTime={value} title={value}>
          {label}
        </time>
      );
    }
  }
  let text: string;
  if (typeof value === 'number' && kind !== 'text') {
    const decimals = display?.decimals;
    const precision =
      decimals == null || !Number.isFinite(decimals)
        ? undefined
        : Math.max(0, Math.min(20, Math.trunc(decimals)));
    text = new Intl.NumberFormat(undefined, {
      minimumFractionDigits: precision,
      maximumFractionDigits: precision ?? 20,
    }).format(value);
  } else if (typeof value === 'boolean') {
    text = value ? 'Yes' : 'No';
  } else if (typeof value === 'string') {
    text = value;
  } else {
    text = JSON.stringify(value);
  }
  return (
    <span className="whitespace-pre-wrap break-words">
      {display?.prefix}
      {text}
      {display?.suffix}
    </span>
  );
}
