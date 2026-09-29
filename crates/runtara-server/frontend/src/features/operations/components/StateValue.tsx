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
  kind?: 'text' | 'number' | 'date' | 'datetime' | 'relative';
  decimals?: number;
  prefix?: string;
  suffix?: string;
}

export function stateLabel(key: string, field?: StateField) {
  return (
    field?.label ??
    key.replace(/[_-]/g, ' ').replace(/^./, (c) => c.toUpperCase())
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
            : date.toLocaleString();
      return (
        <time dateTime={value} title={value}>
          {label}
        </time>
      );
    }
  }
  let text: string;
  if (typeof value === 'number') {
    const decimals = display?.decimals;
    const precision =
      decimals === undefined || !Number.isFinite(decimals)
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
