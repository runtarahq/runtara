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
} from '../../../types';

const KINDS: Array<{ value: ReportChartKind; label: string }> = [
  { value: 'bar', label: 'Bar' },
  { value: 'line', label: 'Line' },
  { value: 'area', label: 'Area' },
  { value: 'pie', label: 'Pie' },
  { value: 'donut', label: 'Donut' },
];

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
  const fields = schema?.columns.map((c) => c.name) ?? [];

  const updateSeries = (next: ReportChartSeries[]) =>
    onChange({ ...block, chart: { ...chart, series: next } });

  return (
    <div className="grid gap-3">
      <div className="grid grid-cols-2 gap-3">
        <div className="grid gap-1.5">
          <Label className="text-xs">Chart kind</Label>
          <Select
            value={chart.kind}
            onValueChange={(value) =>
              onChange({
                ...block,
                chart: { ...chart, kind: value as ReportChartKind },
              })
            }
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
          <Label className="text-xs">X axis</Label>
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
              {chart.x && !fields.includes(chart.x) ? (
                <SelectItem disabled value={chart.x}>
                  {chart.x}
                </SelectItem>
              ) : null}
              {fields.map((field) => (
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
          <Label className="text-xs">Series</Label>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="h-7"
            onClick={() => updateSeries([...series, { field: '' }])}
          >
            <Plus className="mr-1 h-3 w-3" /> Add series
          </Button>
        </div>
        {series.length === 0 ? (
          <p className="text-xs text-muted-foreground">
            No series yet. Add one to plot a value.
          </p>
        ) : (
          <div className="grid gap-2">
            {series.map((entry, index) => (
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
                    <SelectValue placeholder="Field" />
                  </SelectTrigger>
                  <SelectContent>
                    {entry.field && !fields.includes(entry.field) ? (
                      <SelectItem disabled value={entry.field}>
                        {entry.field}
                      </SelectItem>
                    ) : null}
                    {fields.map((field) => (
                      <SelectItem key={field} value={field}>
                        {field}
                      </SelectItem>
                    ))}
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
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
