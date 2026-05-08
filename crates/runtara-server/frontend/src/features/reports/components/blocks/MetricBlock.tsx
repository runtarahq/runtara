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

  return (
    <div className="rounded-lg border bg-background p-4">
      <p className="text-sm font-medium text-muted-foreground">{label}</p>
      <p className="mt-2 text-3xl font-semibold tracking-normal text-foreground">
        {formatCellValue(data.value, format ?? undefined) || '0'}
      </p>
    </div>
  );
}
