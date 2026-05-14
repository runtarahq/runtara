import { useMemo } from 'react';
import ReactMarkdown from 'react-markdown';
import { BarChart3, LineChart, PieChart, AreaChart } from 'lucide-react';
import { cn } from '@/lib/utils';
import { humanizeFieldName, formatCellValue } from '../../../utils';
import {
  ReportBlockDefinition,
  ReportBlockResult,
  ReportDatasetDefinition,
  ReportTableColumn,
} from '../../../types';
import { ChartBlock } from '../../blocks/ChartBlock';
import { MetricBlock } from '../../blocks/MetricBlock';
import {
  datasetFieldLabel,
  datasetQueryOutputFields,
} from '../../../datasetBlocks';
import { WizardBlock, WizardColumnFormat } from '../wizardTypes';

interface BlockPreviewProps {
  block: WizardBlock;
  result?: ReportBlockResult;
  datasets?: ReportDatasetDefinition[];
}

/** When the block uses a dataset, derive a flat WizardBlock-shaped projection
 *  the rest of the preview already understands: fields = dim ∪ measures, with
 *  dataset-supplied labels/formats stuffed into fieldConfigs. */
function projectDatasetBlock(
  block: WizardBlock,
  datasets: ReportDatasetDefinition[] | undefined
): WizardBlock {
  const query = block.dataset;
  if (!query) return block;
  const dataset = datasets?.find((candidate) => candidate.id === query.id);
  const fields = datasetQueryOutputFields(query);
  const fieldConfigs: WizardBlock['fieldConfigs'] = {};
  for (const field of fields) {
    const dimension = dataset?.dimensions.find((d) => d.field === field);
    const measure = dataset?.measures.find((m) => m.id === field);
    const format = (dimension?.format ?? measure?.format) as
      | WizardColumnFormat
      | undefined;
    fieldConfigs[field] = {
      label: datasetFieldLabel(dataset, field),
      ...(format ? { format } : {}),
    };
  }
  const firstDimension = (query.dimensions ?? [])[0];
  const firstMeasure = (query.measures ?? [])[0];
  return {
    ...block,
    fields,
    fieldConfigs,
    chartGroupBy: block.chartGroupBy ?? firstDimension ?? fields[0],
    metricField: block.metricField ?? firstMeasure,
  };
}

const SAMPLE_VALUES: Record<WizardColumnFormat | 'plain', string> = {
  plain: '—',
  number: '1,234',
  decimal: '1,234.56',
  currency: '$1,234.56',
  percent: '12.4%',
  date: '2026-05-14',
  datetime: '2026-05-14 09:30',
  pill: 'active',
  bar_indicator: '••••○',
  boolean: 'true',
};

function sampleFor(format?: WizardColumnFormat): string {
  return SAMPLE_VALUES[format ?? 'plain'];
}

function pillClass(format: WizardColumnFormat | undefined) {
  return cn(
    'inline-flex items-center rounded-full border px-2 py-0.5 text-[10px] font-medium',
    format === 'pill' &&
      'border-emerald-300/60 bg-emerald-50 text-emerald-800 dark:bg-emerald-950/40 dark:text-emerald-200'
  );
}

/** Convert a WizardBlock to a minimal ReportBlockDefinition so the existing
 *  block components (MetricBlock, ChartBlock) can render real preview data. */
function wizardBlockToDefinition(block: WizardBlock): ReportBlockDefinition {
  const columns: ReportTableColumn[] = block.fields.map((field) => {
    const cfg = block.fieldConfigs?.[field];
    return {
      field,
      label: cfg?.label || humanizeFieldName(field),
      ...(cfg?.format ? { format: cfg.format } : {}),
      ...(cfg?.pillVariants ? { pillVariants: cfg.pillVariants } : {}),
    };
  });

  const seriesFields = block.fields.length > 0 ? block.fields : ['value'];

  return {
    id: block.id,
    type: block.type,
    title: block.title,
    source: { schema: block.schema ?? '', mode: 'filter' },
    metric:
      block.type === 'metric'
        ? {
            valueField: 'value',
            label: block.title,
            format: block.metricFormat ?? 'number',
          }
        : undefined,
    chart:
      block.type === 'chart'
        ? {
            kind: block.chartKind ?? 'bar',
            x: block.chartGroupBy || seriesFields[0],
            series: seriesFields.map((field) => ({
              field,
              label: block.fieldConfigs?.[field]?.label || humanizeFieldName(field),
            })),
          }
        : undefined,
    table: block.type === 'table' ? { columns } : undefined,
    card:
      block.type === 'card'
        ? {
            groups: [
              {
                id: 'main',
                fields: columns.map((column) => ({
                  field: column.field,
                  label: column.label,
                  ...(column.format ? { format: column.format } : {}),
                })),
              },
            ],
          }
        : undefined,
  };
}

function hasRealData(result: ReportBlockResult | undefined): boolean {
  return Boolean(
    result &&
      result.status === 'ready' &&
      result.data !== undefined &&
      result.data !== null
  );
}

export function BlockPreview({
  block: inputBlock,
  result,
  datasets,
}: BlockPreviewProps) {
  // Dataset-mode blocks store the query on `block.dataset` and leave
  // `fields`/`fieldConfigs` empty. Project them into the wizard shape the rest
  // of the preview already understands so columns/series render meaningfully.
  const block = useMemo(
    () => projectDatasetBlock(inputBlock, datasets),
    [inputBlock, datasets]
  );
  const blockDefinition = useMemo(
    () => wizardBlockToDefinition(block),
    [block]
  );

  // Markdown is local — render the typed content directly, no need for results.
  if (block.type === 'markdown') {
    const content = block.markdownContent?.trim();
    if (!content) {
      return (
        <p className="px-3 py-2 text-xs italic text-muted-foreground">
          Empty markdown — click to add content.
        </p>
      );
    }
    return (
      <div className="prose prose-sm max-w-none px-3 py-2 text-sm dark:prose-invert">
        <ReactMarkdown>{content}</ReactMarkdown>
      </div>
    );
  }

  // Block-level errors / loading: fall through to sample so the editor still
  // looks complete. Status indicator could come later as a small chip.
  const useReal = hasRealData(result);

  if (block.type === 'metric') {
    if (useReal && result) {
      return (
        <div className="p-3">
          <MetricBlock block={blockDefinition} result={result} />
        </div>
      );
    }
    const sample = sampleFor(block.metricFormat ?? 'number');
    return (
      <div className="grid gap-1 px-3 py-3">
        <span className="text-[11px] uppercase tracking-wider text-muted-foreground">
          {block.title || 'Metric'}
        </span>
        <span className="text-2xl font-bold tabular-nums">{sample}</span>
        <span className="text-[10px] text-muted-foreground">
          {(block.metricAggregate ?? 'count').toUpperCase()}
          {block.metricField ? ` · ${block.metricField}` : ''}
        </span>
      </div>
    );
  }

  if (block.type === 'chart') {
    if (useReal && result) {
      return (
        <div className="p-3">
          <ChartBlock block={blockDefinition} result={result} />
        </div>
      );
    }
    const Icon =
      block.chartKind === 'line'
        ? LineChart
        : block.chartKind === 'area'
          ? AreaChart
          : block.chartKind === 'pie' || block.chartKind === 'donut'
            ? PieChart
            : BarChart3;
    const seriesLabels = (block.fields ?? []).map(
      (f) => block.fieldConfigs?.[f]?.label || humanizeFieldName(f)
    );
    return (
      <div className="grid gap-2 px-3 py-3">
        <ChartSketch kind={block.chartKind ?? 'bar'} />
        <div className="flex items-center justify-between gap-2 text-[11px] text-muted-foreground">
          <div className="flex items-center gap-1.5">
            <Icon className="h-3.5 w-3.5" />
            <span>
              by{' '}
              <span className="font-medium text-foreground">
                {block.chartGroupBy
                  ? humanizeFieldName(block.chartGroupBy)
                  : 'no field'}
              </span>
            </span>
          </div>
          {seriesLabels.length > 0 ? (
            <div className="flex flex-wrap gap-1">
              {seriesLabels.map((label) => (
                <span
                  key={label}
                  className="inline-flex items-center gap-1 rounded-full bg-muted px-2 py-0.5 text-[10px] font-medium"
                >
                  <span className="h-1.5 w-1.5 rounded-full bg-primary" />
                  {label}
                </span>
              ))}
            </div>
          ) : null}
        </div>
      </div>
    );
  }

  if (block.type === 'table') {
    if (block.fields.length === 0) {
      return (
        <p className="px-3 py-3 text-xs italic text-muted-foreground">
          No columns yet — click to configure.
        </p>
      );
    }
    const columns = block.fields.map((field) => ({
      field,
      label: block.fieldConfigs?.[field]?.label || humanizeFieldName(field),
      format: block.fieldConfigs?.[field]?.format,
    }));
    const realRows = useReal ? extractTableRows(result, columns) : null;
    return (
      <div className="w-full max-w-full overflow-x-auto">
        <table className="w-full min-w-full text-xs">
          <thead className="bg-muted/40 text-muted-foreground">
            <tr>
              {columns.map((col) => (
                <th
                  key={col.field}
                  className="truncate px-3 py-1.5 text-left font-semibold uppercase tracking-wider"
                >
                  {col.label}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {(realRows && realRows.length > 0
              ? realRows.slice(0, 5)
              : [0, 1, 2].map(() => null)
            ).map((row, rowIndex) => (
              <tr key={rowIndex} className="border-t">
                {columns.map((col) => {
                  const realValue = row ? row[col.field] : undefined;
                  const display =
                    row !== null && realValue !== undefined
                      ? formatCellValue(realValue, col.format ?? undefined) ||
                        '—'
                      : sampleFor(col.format);
                  return (
                    <td
                      key={col.field}
                      className="truncate px-3 py-1.5 text-foreground/80"
                    >
                      {col.format === 'pill' ? (
                        <span className={pillClass('pill')}>{display}</span>
                      ) : (
                        display
                      )}
                    </td>
                  );
                })}
              </tr>
            ))}
            {realRows && realRows.length === 0 ? (
              <tr>
                <td
                  colSpan={columns.length}
                  className="px-3 py-3 text-center text-[10px] italic text-muted-foreground"
                >
                  No rows
                </td>
              </tr>
            ) : null}
          </tbody>
        </table>
      </div>
    );
  }

  // card
  if (block.fields.length === 0) {
    return (
      <p className="px-3 py-3 text-xs italic text-muted-foreground">
        No fields yet — click to configure.
      </p>
    );
  }
  const realRow = useReal ? extractCardRow(result) : undefined;
  return (
    <dl className="grid gap-1.5 px-3 py-3 sm:grid-cols-2">
      {block.fields.map((field) => {
        const cfg = block.fieldConfigs?.[field];
        const label = cfg?.label || humanizeFieldName(field);
        const realValue = realRow ? realRow[field] : undefined;
        const display =
          realRow && realValue !== undefined
            ? formatCellValue(realValue, cfg?.format ?? undefined) || '—'
            : sampleFor(cfg?.format);
        return (
          <div key={field} className="grid gap-0.5">
            <dt className="text-[10px] uppercase tracking-wider text-muted-foreground">
              {label}
            </dt>
            <dd className="text-xs font-medium">
              {cfg?.format === 'pill' ? (
                <span className={pillClass('pill')}>{display}</span>
              ) : (
                display
              )}
            </dd>
          </div>
        );
      })}
    </dl>
  );
}

/** Pull rows out of a table block's result. Tables use {columns, rows} pairs. */
function extractTableRows(
  result: ReportBlockResult | undefined,
  _columns: Array<{ field: string }>
): Array<Record<string, unknown>> | null {
  if (!result || !result.data) return null;
  const data = result.data as {
    columns?: Array<{ field?: string; key?: string }>;
    rows?: unknown[][] | Array<Record<string, unknown>>;
  };
  if (!Array.isArray(data.rows) || data.rows.length === 0) return [];
  // Rows come back as either arrays (positional) or objects.
  if (Array.isArray(data.rows[0])) {
    const dataColumns = data.columns ?? [];
    return (data.rows as unknown[][]).map((row) =>
      Object.fromEntries(
        dataColumns.map((col, index) => [
          col.field ?? col.key ?? `col_${index}`,
          row[index],
        ])
      )
    );
  }
  // Already object-shaped; pass through (filtered to known columns).
  return data.rows as Array<Record<string, unknown>>;
}

/** Card blocks typically render one row's worth of fields. */
function extractCardRow(
  result: ReportBlockResult | undefined
): Record<string, unknown> | undefined {
  const rows = extractTableRows(result, []);
  if (!rows || rows.length === 0) return undefined;
  return rows[0];
}

function ChartSketch({
  kind,
}: {
  kind: 'bar' | 'line' | 'area' | 'pie' | 'donut';
}) {
  if (kind === 'pie' || kind === 'donut') {
    return (
      <svg viewBox="0 0 80 40" className="h-12 w-full">
        <circle
          cx="40"
          cy="20"
          r="14"
          className="fill-primary/15 stroke-primary/40"
          strokeWidth="2"
        />
        {kind === 'donut' ? (
          <circle cx="40" cy="20" r="6" className="fill-background" />
        ) : null}
        <path
          d="M40 20 L40 6 A14 14 0 0 1 53 22 Z"
          className="fill-primary/40"
        />
        <path
          d="M40 20 L53 22 A14 14 0 0 1 33 32 Z"
          className="fill-primary/25"
        />
      </svg>
    );
  }
  if (kind === 'line' || kind === 'area') {
    const path = 'M2 30 L14 22 L26 26 L38 14 L50 18 L62 10 L74 14';
    return (
      <svg viewBox="0 0 80 40" className="h-12 w-full">
        {kind === 'area' ? (
          <path d={`${path} L74 38 L2 38 Z`} className="fill-primary/15" />
        ) : null}
        <path
          d={path}
          fill="none"
          stroke="currentColor"
          className="text-primary"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
    );
  }
  const bars = [16, 28, 12, 24, 32, 18, 22];
  return (
    <svg viewBox="0 0 80 40" className="h-12 w-full">
      {bars.map((h, i) => (
        <rect
          key={i}
          x={2 + i * 11}
          y={38 - h}
          width="8"
          height={h}
          className="fill-primary/60"
          rx="1"
        />
      ))}
    </svg>
  );
}
