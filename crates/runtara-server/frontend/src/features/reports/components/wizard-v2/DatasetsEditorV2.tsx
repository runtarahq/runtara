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
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from '@/shared/components/ui/card';
import { Plus, Trash2 } from 'lucide-react';
import { Schema } from '@/generated/RuntaraRuntimeApi';
import {
  ReportAggregateFn,
  ReportDatasetDefinition,
  ReportDatasetDimension,
  ReportDatasetFieldType,
  ReportDatasetMeasure,
  ReportDatasetValueFormat,
  ReportDefinition,
} from '../../types';
import { humanizeFieldName } from '../../utils';

interface DatasetsEditorV2Props {
  definition: ReportDefinition;
  schemas: Schema[];
  onChange: (definition: ReportDefinition) => void;
}

const DIMENSION_TYPES: Array<{
  value: ReportDatasetFieldType;
  label: string;
}> = [
  { value: 'string', label: 'String' },
  { value: 'number', label: 'Number' },
  { value: 'decimal', label: 'Decimal' },
  { value: 'boolean', label: 'Boolean' },
  { value: 'date', label: 'Date' },
  { value: 'datetime', label: 'Date + time' },
  { value: 'json', label: 'JSON' },
];

const DIMENSION_FORMATS: Array<{
  value: ReportDatasetValueFormat;
  label: string;
}> = [
  { value: 'string', label: 'String' },
  { value: 'number', label: 'Number' },
  { value: 'decimal', label: 'Decimal' },
  { value: 'currency', label: 'Currency' },
  { value: 'percent', label: 'Percent' },
  { value: 'boolean', label: 'Boolean' },
  { value: 'date', label: 'Date' },
  { value: 'datetime', label: 'Date + time' },
];

const AGGREGATE_OPS: Array<{ value: ReportAggregateFn; label: string }> = [
  { value: 'count', label: 'Count' },
  { value: 'sum', label: 'Sum' },
  { value: 'avg', label: 'Average' },
  { value: 'min', label: 'Min' },
  { value: 'max', label: 'Max' },
  { value: 'first_value', label: 'First value' },
  { value: 'last_value', label: 'Last value' },
  { value: 'percentile_cont', label: 'Percentile (continuous)' },
  { value: 'percentile_disc', label: 'Percentile (discrete)' },
  { value: 'stddev_samp', label: 'Std dev (sample)' },
  { value: 'var_samp', label: 'Variance (sample)' },
];

const FORMAT_PLAIN = '__plain__';

function newDataset(): ReportDatasetDefinition {
  const id = `dataset_${Math.random().toString(36).slice(2, 7)}`;
  return {
    id,
    label: 'New dataset',
    source: { schema: '' },
    dimensions: [],
    measures: [],
  };
}

function newDimension(field: string): ReportDatasetDimension {
  return {
    field,
    label: humanizeFieldName(field),
    type: 'string',
  };
}

function newMeasure(): ReportDatasetMeasure {
  return {
    id: `measure_${Math.random().toString(36).slice(2, 7)}`,
    label: 'New measure',
    op: 'count',
    format: 'number',
  };
}

export function DatasetsEditorV2({
  definition,
  schemas,
  onChange,
}: DatasetsEditorV2Props) {
  const datasets = definition.datasets ?? [];

  const updateDatasets = (next: ReportDatasetDefinition[]) =>
    onChange({ ...definition, datasets: next });

  const updateDataset = (
    id: string,
    updater: (dataset: ReportDatasetDefinition) => ReportDatasetDefinition
  ) => updateDatasets(datasets.map((d) => (d.id === id ? updater(d) : d)));

  return (
    <div className="grid gap-3">
      {datasets.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          No datasets yet. Datasets pre-aggregate data so multiple blocks can
          share a query.
        </p>
      ) : (
        <div className="grid gap-3">
          {datasets.map((dataset) => (
            <DatasetCard
              key={dataset.id}
              dataset={dataset}
              schemas={schemas}
              onChange={(updater) => updateDataset(dataset.id, updater)}
              onDelete={() =>
                updateDatasets(datasets.filter((d) => d.id !== dataset.id))
              }
            />
          ))}
        </div>
      )}
      <div>
        <Button
          type="button"
          variant="outline"
          onClick={() => updateDatasets([...datasets, newDataset()])}
        >
          <Plus className="mr-1 h-3.5 w-3.5" /> Add dataset
        </Button>
      </div>
    </div>
  );
}

interface DatasetCardProps {
  dataset: ReportDatasetDefinition;
  schemas: Schema[];
  onChange: (
    updater: (dataset: ReportDatasetDefinition) => ReportDatasetDefinition
  ) => void;
  onDelete: () => void;
}

function DatasetCard({
  dataset,
  schemas,
  onChange,
  onDelete,
}: DatasetCardProps) {
  const schema = schemas.find((s) => s.name === dataset.source.schema);
  const fields = schema?.columns.map((c) => c.name) ?? [];
  const dimensions = dataset.dimensions;
  const measures = dataset.measures;

  return (
    <Card>
      <CardHeader className="flex flex-row items-center justify-between gap-2 space-y-0 py-3">
        <CardTitle className="text-sm">{dataset.label}</CardTitle>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-7 w-7 text-destructive"
          onClick={onDelete}
        >
          <Trash2 className="h-3.5 w-3.5" />
        </Button>
      </CardHeader>
      <CardContent className="grid gap-4 pt-0">
        <div className="grid grid-cols-2 gap-3">
          <div className="grid gap-1.5">
            <Label className="text-xs">Label</Label>
            <Input
              value={dataset.label}
              onChange={(event) =>
                onChange((d) => ({ ...d, label: event.target.value }))
              }
            />
          </div>
          <div className="grid gap-1.5">
            <Label className="text-xs">Schema</Label>
            <Select
              value={dataset.source.schema || ''}
              onValueChange={(value) =>
                onChange((d) => ({
                  ...d,
                  source: { ...d.source, schema: value },
                }))
              }
            >
              <SelectTrigger className="h-9">
                <SelectValue placeholder="Pick a schema" />
              </SelectTrigger>
              <SelectContent>
                {schemas.map((s) => (
                  <SelectItem key={s.name} value={s.name}>
                    {s.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        </div>

        <div className="grid gap-1.5">
          <div className="flex items-center justify-between">
            <Label className="text-xs">Dimensions ({dimensions.length})</Label>
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-7"
              disabled={fields.length === 0}
              onClick={() => {
                const used = new Set(dimensions.map((d) => d.field));
                const next = fields.find((f) => !used.has(f));
                if (!next) return;
                onChange((d) => ({
                  ...d,
                  dimensions: [...d.dimensions, newDimension(next)],
                }));
              }}
            >
              <Plus className="mr-1 h-3 w-3" /> Add dimension
            </Button>
          </div>
          {dimensions.length === 0 ? (
            <p className="text-xs text-muted-foreground">
              No dimensions yet.
            </p>
          ) : (
            <div className="grid gap-2">
              {dimensions.map((dim, index) => (
                <div
                  key={index}
                  className="grid grid-cols-[1fr_1fr_120px_120px_minmax(0,auto)] items-center gap-2 rounded border p-2"
                >
                  <Select
                    value={dim.field || ''}
                    onValueChange={(value) =>
                      onChange((d) => ({
                        ...d,
                        dimensions: d.dimensions.map((x, i) =>
                          i === index ? { ...x, field: value } : x
                        ),
                      }))
                    }
                  >
                    <SelectTrigger className="h-8 text-xs">
                      <SelectValue placeholder="Field" />
                    </SelectTrigger>
                    <SelectContent>
                      {dim.field && !fields.includes(dim.field) ? (
                        <SelectItem disabled value={dim.field}>
                          {dim.field}
                        </SelectItem>
                      ) : null}
                      {fields.map((f) => (
                        <SelectItem key={f} value={f}>
                          {f}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <Input
                    value={dim.label}
                    className="h-8 text-xs"
                    placeholder="Label"
                    onChange={(event) =>
                      onChange((d) => ({
                        ...d,
                        dimensions: d.dimensions.map((x, i) =>
                          i === index
                            ? { ...x, label: event.target.value }
                            : x
                        ),
                      }))
                    }
                  />
                  <Select
                    value={dim.type}
                    onValueChange={(value) =>
                      onChange((d) => ({
                        ...d,
                        dimensions: d.dimensions.map((x, i) =>
                          i === index
                            ? { ...x, type: value as ReportDatasetFieldType }
                            : x
                        ),
                      }))
                    }
                  >
                    <SelectTrigger className="h-8 text-xs">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {DIMENSION_TYPES.map((option) => (
                        <SelectItem key={option.value} value={option.value}>
                          {option.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <Select
                    value={dim.format ?? FORMAT_PLAIN}
                    onValueChange={(value) =>
                      onChange((d) => ({
                        ...d,
                        dimensions: d.dimensions.map((x, i) =>
                          i === index
                            ? {
                                ...x,
                                format:
                                  value === FORMAT_PLAIN
                                    ? null
                                    : (value as ReportDatasetValueFormat),
                              }
                            : x
                        ),
                      }))
                    }
                  >
                    <SelectTrigger className="h-8 text-xs">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value={FORMAT_PLAIN}>Plain</SelectItem>
                      {DIMENSION_FORMATS.map((option) => (
                        <SelectItem key={option.value} value={option.value}>
                          {option.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="h-8 w-8"
                    onClick={() =>
                      onChange((d) => ({
                        ...d,
                        dimensions: d.dimensions.filter((_, i) => i !== index),
                      }))
                    }
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </Button>
                </div>
              ))}
            </div>
          )}
        </div>

        <div className="grid gap-1.5">
          <div className="flex items-center justify-between">
            <Label className="text-xs">Measures ({measures.length})</Label>
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-7"
              onClick={() =>
                onChange((d) => ({
                  ...d,
                  measures: [...d.measures, newMeasure()],
                }))
              }
            >
              <Plus className="mr-1 h-3 w-3" /> Add measure
            </Button>
          </div>
          {measures.length === 0 ? (
            <p className="text-xs text-muted-foreground">No measures yet.</p>
          ) : (
            <div className="grid gap-2">
              {measures.map((measure, index) => (
                <div
                  key={measure.id}
                  className="grid gap-2 rounded border p-2"
                >
                  <div className="grid grid-cols-[1fr_1fr_minmax(0,auto)] items-center gap-2">
                    <Input
                      value={measure.id}
                      className="h-8 text-xs"
                      placeholder="ID"
                      onChange={(event) =>
                        onChange((d) => ({
                          ...d,
                          measures: d.measures.map((x, i) =>
                            i === index ? { ...x, id: event.target.value } : x
                          ),
                        }))
                      }
                    />
                    <Input
                      value={measure.label}
                      className="h-8 text-xs"
                      placeholder="Label"
                      onChange={(event) =>
                        onChange((d) => ({
                          ...d,
                          measures: d.measures.map((x, i) =>
                            i === index
                              ? { ...x, label: event.target.value }
                              : x
                          ),
                        }))
                      }
                    />
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className="h-8 w-8"
                      onClick={() =>
                        onChange((d) => ({
                          ...d,
                          measures: d.measures.filter((_, i) => i !== index),
                        }))
                      }
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </Button>
                  </div>
                  <div className="grid grid-cols-4 gap-2">
                    <Select
                      value={measure.op}
                      onValueChange={(value) =>
                        onChange((d) => ({
                          ...d,
                          measures: d.measures.map((x, i) =>
                            i === index
                              ? { ...x, op: value as ReportAggregateFn }
                              : x
                          ),
                        }))
                      }
                    >
                      <SelectTrigger className="h-8 text-xs">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {AGGREGATE_OPS.map((option) => (
                          <SelectItem key={option.value} value={option.value}>
                            {option.label}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                    <Select
                      value={measure.field ?? ''}
                      onValueChange={(value) =>
                        onChange((d) => ({
                          ...d,
                          measures: d.measures.map((x, i) =>
                            i === index ? { ...x, field: value || null } : x
                          ),
                        }))
                      }
                      disabled={measure.op === 'count'}
                    >
                      <SelectTrigger className="h-8 text-xs">
                        <SelectValue placeholder="Field" />
                      </SelectTrigger>
                      <SelectContent>
                        {measure.field && !fields.includes(measure.field) ? (
                          <SelectItem disabled value={measure.field}>
                            {measure.field}
                          </SelectItem>
                        ) : null}
                        {fields.map((f) => (
                          <SelectItem key={f} value={f}>
                            {f}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                    <Select
                      value={measure.format}
                      onValueChange={(value) =>
                        onChange((d) => ({
                          ...d,
                          measures: d.measures.map((x, i) =>
                            i === index
                              ? {
                                  ...x,
                                  format: value as ReportDatasetValueFormat,
                                }
                              : x
                          ),
                        }))
                      }
                    >
                      <SelectTrigger className="h-8 text-xs">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {DIMENSION_FORMATS.map((option) => (
                          <SelectItem key={option.value} value={option.value}>
                            {option.label}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                    <label className="flex items-center gap-1.5 text-xs">
                      <input
                        type="checkbox"
                        checked={Boolean(measure.distinct)}
                        onChange={(event) =>
                          onChange((d) => ({
                            ...d,
                            measures: d.measures.map((x, i) =>
                              i === index
                                ? { ...x, distinct: event.target.checked }
                                : x
                            ),
                          }))
                        }
                      />
                      Distinct
                    </label>
                  </div>
                  {measure.op === 'percentile_cont' ||
                  measure.op === 'percentile_disc' ? (
                    <div className="grid gap-1.5">
                      <Label className="text-xs">Percentile (0–1)</Label>
                      <Input
                        type="number"
                        min={0}
                        max={1}
                        step={0.01}
                        value={measure.percentile ?? ''}
                        className="h-8 text-xs"
                        onChange={(event) => {
                          const value = event.target.value
                            ? parseFloat(event.target.value)
                            : null;
                          onChange((d) => ({
                            ...d,
                            measures: d.measures.map((x, i) =>
                              i === index ? { ...x, percentile: value } : x
                            ),
                          }));
                        }}
                      />
                    </div>
                  ) : null}
                </div>
              ))}
            </div>
          )}
        </div>

        <div className="grid gap-1.5">
          <Label className="text-xs">Time dimension (optional)</Label>
          <Select
            value={dataset.timeDimension ?? FORMAT_PLAIN}
            onValueChange={(value) =>
              onChange((d) => ({
                ...d,
                timeDimension: value === FORMAT_PLAIN ? null : value,
              }))
            }
          >
            <SelectTrigger className="h-8 text-xs">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={FORMAT_PLAIN}>None</SelectItem>
              {dimensions
                .filter((d) => d.type === 'date' || d.type === 'datetime')
                .map((d) => (
                  <SelectItem key={d.field} value={d.field}>
                    {d.field}
                  </SelectItem>
                ))}
            </SelectContent>
          </Select>
        </div>
      </CardContent>
    </Card>
  );
}
