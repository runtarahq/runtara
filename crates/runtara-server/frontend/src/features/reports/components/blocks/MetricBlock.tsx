import { ReportBlockDefinition, ReportBlockResult } from '../../types';
import { formatCellValue } from '../../utils';

type MetricData = {
  value?: unknown;
  label?: string | null;
  format?: string | null;
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

  return (
    <div className="rounded-lg border bg-background p-4">
      <p className="text-sm font-medium text-muted-foreground">{label}</p>
      <p className="mt-2 text-3xl font-semibold tracking-normal text-foreground">
        {formatCellValue(data.value, format ?? undefined) || '0'}
      </p>
    </div>
  );
}
