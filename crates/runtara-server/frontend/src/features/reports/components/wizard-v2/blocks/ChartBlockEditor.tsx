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
import { Schema } from '@/generated/RuntaraRuntimeApi';
import {
  ReportBlockDefinition,
  ReportChartKind,
  ReportChartSeries,
  ReportSource,
} from '../../../types';
import { SourceAggregatesEditor } from './SourceAggregatesEditor';

const KINDS: Array<{ value: ReportChartKind; label: string }> = [
  { value: 'bar', label: 'Bar' },
  { value: 'line', label: 'Line' },
  { value: 'area', label: 'Area' },
  { value: 'pie', label: 'Pie' },
  { value: 'donut', label: 'Donut' },
  { value: 'scatter', label: 'Scatter' },
];

// Sentinel for the "no field selected" option in the optional scatter selects
// (Radix Select cannot use an empty string as an item value).
const NONE_VALUE = '__none__';

interface ChartBlockEditorProps {
  block: ReportBlockDefinition;
  schemas: Schema[];
  onChange: (block: ReportBlockDefinition) => void;
}

export function ChartBlockEditor({
  block,
  schemas,
  onChange,
}: ChartBlockEditorProps) {
  const chart = block.chart ?? { kind: 'bar', x: '', series: [] };
  const series: ReportChartSeries[] = chart.series ?? [];
  const schemaName = block.source?.schema;
  const schema = schemas.find((s) => s.name === schemaName);
  const schemaFields = schema?.columns.map((c) => c.name) ?? [];
  const aggregateAliases = (block.source?.aggregates ?? []).map(
    (agg) => agg.alias
  );
  const isScatter = chart.kind === 'scatter';
  // Phase 11 follow-up: chart series.field references aggregate aliases.
  // The X axis usually picks one of the group-by fields (which are schema
  // columns). Both pickers offer the relevant set so the user can wire
  // chart → source aggregates → schema fields correctly.
  const seriesFieldOptions = aggregateAliases;
  const groupByFields = block.source?.groupBy ?? [];
  // Categorical candidates: group-by dimensions first, then remaining schema
  // columns. Used for the bar/line/area X axis and the scatter "color by".
  const categoricalFieldOptions = [
    ...groupByFields,
    ...schemaFields.filter((f) => !groupByFields.includes(f)),
  ];
  // Scatter's X is numeric, so it also offers aggregate aliases (e.g. avg(price)),
  // not just categorical dimensions.
  const xFieldOptions = isScatter
    ? [
        ...aggregateAliases,
        ...categoricalFieldOptions.filter((f) => !aggregateAliases.includes(f)),
      ]
    : categoricalFieldOptions;
  // Bubble radius must be numeric → aggregate aliases only.
  const sizeFieldOptions = aggregateAliases;
  // Color-by partitions points into clouds → categorical dimensions.
  const groupByOptions = categoricalFieldOptions;

  const updateSeries = (next: ReportChartSeries[]) =>
    onChange({ ...block, chart: { ...chart, series: next } });

  const updateSource = (next: ReportSource) =>
    onChange({ ...block, source: next });

  return (
    <div className="grid gap-3">
      <SourceAggregatesEditor
        source={block.source}
        schemas={schemas}
        onChange={updateSource}
      />
      <div className="grid grid-cols-2 gap-3">
        <div className="grid gap-1.5">
          <Label className="text-xs">Chart kind</Label>
          <Select
            value={chart.kind}
            onValueChange={(value) => {
              const kind = value as ReportChartKind;
              // Scatter needs a Y field (series[0]); seed an empty row so the
              // "Y axis" picker renders immediately after switching.
              const nextSeries =
                kind === 'scatter' && series.length === 0
                  ? [{ field: '' }]
                  : series;
              onChange({
                ...block,
                chart: { ...chart, kind, series: nextSeries },
              });
            }}
          >
            <SelectTrigger className="h-9">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {KINDS.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        <div className="grid gap-1.5">
          <Label className="text-xs">
            {isScatter ? 'X axis (numeric)' : 'X axis'}
          </Label>
          <Select
            value={chart.x || ''}
            onValueChange={(value) =>
              onChange({ ...block, chart: { ...chart, x: value } })
            }
          >
            <SelectTrigger className="h-9">
              <SelectValue placeholder="Pick a field" />
            </SelectTrigger>
            <SelectContent>
              {chart.x && !xFieldOptions.includes(chart.x) ? (
                <SelectItem disabled value={chart.x}>
                  {chart.x}
                </SelectItem>
              ) : null}
              {xFieldOptions.map((field) => (
                <SelectItem key={field} value={field}>
                  {field}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      </div>

      <div className="grid gap-1.5">
        <div className="flex items-center justify-between">
          <Label className="text-xs">{isScatter ? 'Y axis' : 'Series'}</Label>
          {isScatter ? null : (
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-7"
              onClick={() => updateSeries([...series, { field: '' }])}
            >
              <Plus className="mr-1 h-3 w-3" /> Add series
            </Button>
          )}
        </div>
        {series.length === 0 ? (
          <p className="text-xs text-muted-foreground">
            No series yet. Add one to plot a value.
          </p>
        ) : (
          <div className="grid gap-2">
            {(isScatter ? series.slice(0, 1) : series).map((entry, index) => (
              <div
                key={index}
                className="grid grid-cols-[1fr_1fr_minmax(0,auto)] items-center gap-2 rounded border p-2"
              >
                <Select
                  value={entry.field || ''}
                  onValueChange={(value) =>
                    updateSeries(
                      series.map((s, i) =>
                        i === index ? { ...s, field: value } : s
                      )
                    )
                  }
                >
                  <SelectTrigger className="h-8 text-xs">
                    <SelectValue placeholder="Aggregate alias" />
                  </SelectTrigger>
                  <SelectContent>
                    {entry.field &&
                    !seriesFieldOptions.includes(entry.field) ? (
                      <SelectItem disabled value={entry.field}>
                        {entry.field}
                      </SelectItem>
                    ) : null}
                    {seriesFieldOptions.map((field) => (
                      <SelectItem key={field} value={field}>
                        {field}
                      </SelectItem>
                    ))}
                    {seriesFieldOptions.length === 0 ? (
                      <SelectItem disabled value="__no_aggregates__">
                        Add a source aggregate first
                      </SelectItem>
                    ) : null}
                  </SelectContent>
                </Select>
                <Input
                  value={entry.label ?? ''}
                  placeholder="Label"
                  className="h-8 text-xs"
                  onChange={(event) =>
                    updateSeries(
                      series.map((s, i) =>
                        i === index
                          ? { ...s, label: event.target.value || null }
                          : s
                      )
                    )
                  }
                />
                {isScatter ? (
                  <span />
                ) : (
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="h-8 w-8"
                    onClick={() =>
                      updateSeries(series.filter((_, i) => i !== index))
                    }
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </Button>
                )}
              </div>
            ))}
          </div>
        )}
      </div>

      {isScatter ? (
        <div className="grid grid-cols-2 gap-3">
          <div className="grid gap-1.5">
            <Label className="text-xs">Bubble size (optional)</Label>
            <Select
              value={chart.sizeField ?? NONE_VALUE}
              onValueChange={(value) =>
                onChange({
                  ...block,
                  chart: {
                    ...chart,
                    sizeField: value === NONE_VALUE ? undefined : value,
                  },
                })
              }
            >
              <SelectTrigger className="h-9">
                <SelectValue placeholder="None" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={NONE_VALUE}>None</SelectItem>
                {chart.sizeField &&
                !sizeFieldOptions.includes(chart.sizeField) ? (
                  <SelectItem disabled value={chart.sizeField}>
                    {chart.sizeField}
                  </SelectItem>
                ) : null}
                {sizeFieldOptions.map((field) => (
                  <SelectItem key={field} value={field}>
                    {field}
                  </SelectItem>
                ))}
                {sizeFieldOptions.length === 0 ? (
                  <SelectItem disabled value="__no_size_aggregates__">
                    Add a source aggregate first
                  </SelectItem>
                ) : null}
              </SelectContent>
            </Select>
          </div>
          <div className="grid gap-1.5">
            <Label className="text-xs">Color by (optional)</Label>
            <Select
              value={chart.groupBy ?? NONE_VALUE}
              onValueChange={(value) =>
                onChange({
                  ...block,
                  chart: {
                    ...chart,
                    groupBy: value === NONE_VALUE ? undefined : value,
                  },
                })
              }
            >
              <SelectTrigger className="h-9">
                <SelectValue placeholder="None" />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={NONE_VALUE}>None</SelectItem>
                {chart.groupBy && !groupByOptions.includes(chart.groupBy) ? (
                  <SelectItem disabled value={chart.groupBy}>
                    {chart.groupBy}
                  </SelectItem>
                ) : null}
                {groupByOptions.map((field) => (
                  <SelectItem key={field} value={field}>
                    {field}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        </div>
      ) : null}
    </div>
  );
}
