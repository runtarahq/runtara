import { ReportBlockDefinition, ReportBlockResult } from '../../types';
import { formatCellValue } from '../../utils';

type MetricData = {
  value?: unknown;
  label?: string | null;
  format?: string | null;
  missing?: boolean;
  unsatisfiedFilter?: string;
  message?: string;
};

// Above this magnitude a full-precision figure (e.g. a currency total)
// overflows the fixed-width metric card. We render it in compact notation
// ("$78M") so the card scales with value growth, and keep the exact figure
// in a tooltip / aria-label. Smaller values render in full and fit as-is.
const COMPACT_THRESHOLD = 100_000;

export function MetricBlock({
  block,
  result,
}: {
  block: ReportBlockDefinition;
  result: ReportBlockResult;
}) {
  const data = (result.data ?? {}) as MetricData;
  const label = data.label ?? block.metric?.label ?? block.title ?? 'Metric';
  const format = data.format ?? block.metric?.format;

  if (data.missing && data.unsatisfiedFilter) {
    return (
      <div className="rounded-lg border border-dashed bg-muted/20 p-4">
        <p className="text-sm font-medium text-muted-foreground">{label}</p>
        <p className="mt-2 text-xs text-muted-foreground">
          Filter '{data.unsatisfiedFilter}' not set.
        </p>
      </div>
    );
  }

  const full = formatCellValue(data.value, format ?? undefined) || '0';
  const num = coerceNumber(data.value);
  const compactFormat = compactFormatFor(format);
  const compact =
    num !== null && Math.abs(num) >= COMPACT_THRESHOLD && compactFormat
      ? formatCellValue(data.value, compactFormat)
      : null;

  return (
    <div className="rounded-lg border bg-background p-4">
      <p className="text-sm font-medium text-muted-foreground">{label}</p>
      <p
        className="mt-2 text-3xl font-semibold tracking-normal tabular-nums text-foreground"
        title={compact ? full : undefined}
        aria-label={compact ? full : undefined}
      >
        {compact ?? full}
      </p>
    </div>
  );
}

/**
 * Map a base numeric format to its compact variant, or null when the
 * format has no meaningful compact form (dates, percents, bytes, plain
 * strings). Formats carry an optional `:arg` suffix (e.g. `currency:EUR`).
 */
function compactFormatFor(format: string | null | undefined): string | null {
  if (!format) return null;
  const separator = format.indexOf(':');
  const kind = separator === -1 ? format : format.slice(0, separator);
  const arg = separator === -1 ? '' : format.slice(separator + 1);
  if (kind === 'currency') {
    return arg ? `currency_compact:${arg}` : 'currency_compact';
  }
  if (kind === 'number') return 'number_compact';
  return null;
}

function coerceNumber(value: unknown): number | null {
  if (typeof value === 'number') return Number.isFinite(value) ? value : null;
  if (typeof value === 'string' && value.trim() !== '') {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : null;
  }
  return null;
}
