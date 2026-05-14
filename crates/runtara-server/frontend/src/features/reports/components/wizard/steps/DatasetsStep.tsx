import { useState } from 'react';
import { ChevronUp, Plus, Settings2, Trash2 } from 'lucide-react';
import { cn } from '@/lib/utils';
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
import { Schema } from '@/generated/RuntaraRuntimeApi';
import { Checkbox } from '@/shared/components/ui/checkbox';
import {
  ReportAggregateFn,
  ReportDatasetDefinition,
  ReportDatasetFieldType,
  ReportDatasetValueFormat,
} from '../../../types';
import { humanizeFieldName, slugify } from '../../../utils';
import { makeDatasetId, makeMeasureId } from '../wizardTypes';

interface DatasetsStepProps {
  datasets: ReportDatasetDefinition[];
  schemas: Schema[];
  defaultSchema?: string;
  onChange: (next: ReportDatasetDefinition[]) => void;
}

const FIELD_TYPES: Array<{ value: ReportDatasetFieldType; label: string }> = [
  { value: 'string', label: 'String' },
  { value: 'number', label: 'Number' },
  { value: 'decimal', label: 'Decimal' },
  { value: 'boolean', label: 'Boolean' },
  { value: 'date', label: 'Date' },
  { value: 'datetime', label: 'Date + time' },
  { value: 'json', label: 'JSON' },
];

const VALUE_FORMATS: Array<{ value: ReportDatasetValueFormat; label: string }> = [
  { value: 'string', label: 'String' },
  { value: 'number', label: 'Number' },
  { value: 'decimal', label: 'Decimal' },
  { value: 'currency', label: 'Currency' },
  { value: 'percent', label: 'Percent' },
  { value: 'boolean', label: 'Boolean' },
  { value: 'date', label: 'Date' },
  { value: 'datetime', label: 'Date + time' },
];

const MEASURE_OPS: Array<{ value: ReportAggregateFn; label: string }> = [
  { value: 'count', label: 'Count' },
  { value: 'sum', label: 'Sum' },
  { value: 'avg', label: 'Average' },
  { value: 'min', label: 'Min' },
  { value: 'max', label: 'Max' },
  { value: 'first_value', label: 'First value' },
  { value: 'last_value', label: 'Last value' },
  { value: 'percentile_cont', label: 'Percentile (continuous)' },
  { value: 'percentile_disc', label: 'Percentile (discrete)' },
  { value: 'stddev_samp', label: 'Std. deviation (sample)' },
  { value: 'var_samp', label: 'Variance (sample)' },
  { value: 'expr', label: 'Custom expression' },
];

function inferFieldType(
  schema: Schema | undefined,
  field: string
): ReportDatasetFieldType {
  const column = schema?.columns.find((c) => c.name === field);
  switch (column?.type) {
    case 'integer':
      return 'number';
    case 'decimal':
      return 'decimal';
    case 'boolean':
      return 'boolean';
    case 'timestamp':
      return 'datetime';
    case 'json':
      return 'json';
    default:
      return 'string';
  }
}

function defaultFormatForType(
  type: ReportDatasetFieldType
): ReportDatasetValueFormat {
  switch (type) {
    case 'number':
      return 'number';
    case 'decimal':
      return 'decimal';
    case 'boolean':
      return 'boolean';
    case 'date':
      return 'date';
    case 'datetime':
      return 'datetime';
    default:
      return 'string';
  }
}

function seedDataset(
  schemas: Schema[],
  defaultSchema: string | undefined
): ReportDatasetDefinition {
  const schema =
    schemas.find((s) => s.name === defaultSchema) ?? schemas[0];
  const schemaName = schema?.name ?? '';
  const firstField = schema?.columns[0]?.name;
  return {
    id: makeDatasetId(),
    label: 'New dataset',
    source: { schema: schemaName },
    dimensions: firstField
      ? [
          {
            field: firstField,
            label: humanizeFieldName(firstField),
            type: inferFieldType(schema, firstField),
          },
        ]
      : [],
    measures: [
      {
        id: 'total_count',
        label: 'Total count',
        op: 'count',
        format: 'number',
      },
    ],
  };
}

export function DatasetsStep({
  datasets,
  schemas,
  defaultSchema,
  onChange,
}: DatasetsStepProps) {
  const [openId, setOpenId] = useState<string | null>(null);

  function addDataset() {
    const next = seedDataset(schemas, defaultSchema);
    onChange([...datasets, next]);
    setOpenId(next.id);
  }

  function updateDataset(id: string, patch: Partial<ReportDatasetDefinition>) {
    onChange(
      datasets.map((dataset) =>
        dataset.id === id ? { ...dataset, ...patch } : dataset
      )
    );
  }

  function renameDataset(id: string, label: string) {
    const trimmed = label.trim();
    // Keep the slug-style id in sync with the label only while it still looks
    // auto-generated (starts with "dataset_") — otherwise we'd silently rewrite
    // ids that blocks already reference.
    const current = datasets.find((d) => d.id === id);
    const next: Partial<ReportDatasetDefinition> = { label };
    if (current && current.id.startsWith('dataset_') && trimmed) {
      const slug = slugify(trimmed);
      if (slug) next.id = slug;
    }
    updateDataset(id, next);
  }

  function removeDataset(id: string) {
    onChange(datasets.filter((dataset) => dataset.id !== id));
    if (openId === id) setOpenId(null);
  }

  return (
    <div className="grid gap-3">
      {datasets.length === 0 ? (
        <p className="text-xs text-muted-foreground">
          Datasets pre-aggregate a schema into named dimensions and measures —
          like Cube or LookML. Blocks reference one dataset and pick which
          dimensions and measures to query. Skip this section if your blocks
          query schemas directly.
        </p>
      ) : null}

      {datasets.map((dataset) => (
        <DatasetCard
          key={dataset.id}
          dataset={dataset}
          schemas={schemas}
          open={openId === dataset.id}
          onToggle={() =>
            setOpenId(openId === dataset.id ? null : dataset.id)
          }
          onRename={(label) => renameDataset(dataset.id, label)}
          onChange={(patch) => updateDataset(dataset.id, patch)}
          onRemove={() => removeDataset(dataset.id)}
        />
      ))}

      <button
        type="button"
        onClick={addDataset}
        disabled={schemas.length === 0}
        className="flex w-full items-center justify-center gap-1.5 rounded-md border border-dashed bg-muted/10 py-3 text-xs text-muted-foreground transition-colors hover:bg-muted/20 hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50"
      >
        <Plus className="h-4 w-4" />
        <span>Add dataset</span>
      </button>
    </div>
  );
}

function DatasetCard({
  dataset,
  schemas,
  open,
  onToggle,
  onRename,
  onChange,
  onRemove,
}: {
  dataset: ReportDatasetDefinition;
  schemas: Schema[];
  open: boolean;
  onToggle: () => void;
  onRename: (label: string) => void;
  onChange: (patch: Partial<ReportDatasetDefinition>) => void;
  onRemove: () => void;
}) {
  const schema = schemas.find((s) => s.name === dataset.source.schema);
  const schemaFields = schema?.columns.map((c) => c.name) ?? [];
  const summary = `${dataset.dimensions.length} dim · ${dataset.measures.length} measure${dataset.measures.length === 1 ? '' : 's'}`;

  return (
    <article
      className={cn(
        'overflow-hidden rounded-md border bg-background shadow-sm'
      )}
    >
      <div className="flex items-start justify-between gap-2 border-b bg-muted/20 px-3 py-2">
        <div className="min-w-0 flex-1">
          <input
            value={dataset.label}
            placeholder="Untitled dataset"
            onChange={(event) => onRename(event.target.value)}
            className="w-full bg-transparent text-sm font-semibold placeholder:text-muted-foreground focus:outline-none"
            style={{ border: 'none', outline: 'none', boxShadow: 'none' }}
          />
          <div className="flex items-center gap-2 text-[11px] uppercase tracking-wider text-muted-foreground">
            <span className="font-mono normal-case tracking-normal">
              {dataset.id}
            </span>
            <span>·</span>
            <span>{summary}</span>
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-1">
          <Button
            type="button"
            size="icon"
            variant="ghost"
            className="h-7 w-7"
            onClick={onToggle}
            aria-label={open ? 'Collapse dataset' : 'Configure dataset'}
          >
            {open ? (
              <ChevronUp className="h-3.5 w-3.5" />
            ) : (
              <Settings2 className="h-3.5 w-3.5" />
            )}
          </Button>
          <Button
            type="button"
            size="icon"
            variant="ghost"
            className="h-7 w-7"
            onClick={onRemove}
            aria-label="Remove dataset"
          >
            <Trash2 className="h-3.5 w-3.5" />
          </Button>
        </div>
      </div>

      {open ? (
        <div className="grid gap-3 px-3 py-3">
          <div className="grid gap-2 sm:grid-cols-2">
            <div className="grid gap-1.5">
              <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
                Source schema
              </Label>
              <Select
                value={dataset.source.schema || ''}
                onValueChange={(value) =>
                  onChange({
                    source: { ...dataset.source, schema: value },
                    // Drop dimensions/measures whose field no longer exists.
                    dimensions: dataset.dimensions.filter((d) =>
                      schemas
                        .find((s) => s.name === value)
                        ?.columns.some((c) => c.name === d.field)
                    ),
                    measures: dataset.measures.filter(
                      (m) =>
                        !m.field ||
                        schemas
                          .find((s) => s.name === value)
                          ?.columns.some((c) => c.name === m.field)
                    ),
                  })
                }
              >
                <SelectTrigger>
                  <SelectValue placeholder="Select schema" />
                </SelectTrigger>
                <SelectContent>
                  {schemas.map((s) => (
                    <SelectItem key={s.id} value={s.name}>
                      {s.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="grid gap-1.5">
              <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
                Connection (optional)
              </Label>
              <Input
                placeholder="Connection ID — leave empty for default"
                value={dataset.source.connectionId ?? ''}
                onChange={(event) =>
                  onChange({
                    source: {
                      ...dataset.source,
                      connectionId: event.target.value || null,
                    },
                  })
                }
              />
            </div>
            <div className="grid gap-1.5 sm:col-span-2">
              <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
                Time dimension (optional)
              </Label>
              <Select
                value={dataset.timeDimension ?? '__none__'}
                onValueChange={(value) =>
                  onChange({
                    timeDimension: value === '__none__' ? undefined : value,
                  })
                }
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="__none__">No time dimension</SelectItem>
                  {dataset.dimensions
                    .filter(
                      (d) => d.type === 'date' || d.type === 'datetime'
                    )
                    .map((d) => (
                      <SelectItem key={d.field} value={d.field}>
                        {d.label}
                      </SelectItem>
                    ))}
                </SelectContent>
              </Select>
            </div>
          </div>

          <DimensionsEditor
            dimensions={dataset.dimensions}
            schemaFields={schemaFields}
            schema={schema}
            onChange={(dimensions) => onChange({ dimensions })}
          />

          <MeasuresEditor
            measures={dataset.measures}
            schemaFields={schemaFields}
            onChange={(measures) => onChange({ measures })}
          />
        </div>
      ) : null}
    </article>
  );
}

function DimensionsEditor({
  dimensions,
  schemaFields,
  schema,
  onChange,
}: {
  dimensions: ReportDatasetDefinition['dimensions'];
  schemaFields: string[];
  schema: Schema | undefined;
  onChange: (next: ReportDatasetDefinition['dimensions']) => void;
}) {
  const usedFields = new Set(dimensions.map((d) => d.field));
  const availableFields = schemaFields.filter(
    (field) => !usedFields.has(field)
  );

  function addDimension(field: string) {
    const type = inferFieldType(schema, field);
    onChange([
      ...dimensions,
      { field, label: humanizeFieldName(field), type },
    ]);
  }

  function updatePatch(
    index: number,
    patch: Partial<ReportDatasetDefinition['dimensions'][number]>
  ) {
    onChange(
      dimensions.map((dim, i) => (i === index ? { ...dim, ...patch } : dim))
    );
  }

  function removeDimension(index: number) {
    onChange(dimensions.filter((_, i) => i !== index));
  }

  return (
    <div className="grid gap-2">
      <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
        Dimensions
      </Label>
      {dimensions.length === 0 ? (
        <p className="text-xs text-muted-foreground">
          No dimensions yet. Add one below.
        </p>
      ) : (
        <table className="w-full text-sm">
          <thead>
            <tr className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
              <th className="py-1 pr-2 text-left font-semibold">Field</th>
              <th className="py-1 pr-2 text-left font-semibold">Label</th>
              <th className="py-1 pr-2 text-left font-semibold">Type</th>
              <th className="py-1 pr-2 text-left font-semibold">Format</th>
              <th className="w-8 py-1" />
            </tr>
          </thead>
          <tbody>
            {dimensions.map((dim, index) => (
              <tr key={`${dim.field}-${index}`} className="border-t">
                <td className="py-1.5 pr-2 align-middle">
                  <span className="font-mono text-xs">{dim.field}</span>
                </td>
                <td className="py-1.5 pr-2 align-middle">
                  <Input
                    value={dim.label}
                    placeholder={humanizeFieldName(dim.field)}
                    onChange={(event) =>
                      updatePatch(index, { label: event.target.value })
                    }
                    className="h-7"
                  />
                </td>
                <td className="py-1.5 pr-2 align-middle">
                  <Select
                    value={dim.type}
                    onValueChange={(value) =>
                      updatePatch(index, {
                        type: value as ReportDatasetFieldType,
                      })
                    }
                  >
                    <SelectTrigger className="h-7">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {FIELD_TYPES.map((option) => (
                        <SelectItem key={option.value} value={option.value}>
                          {option.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </td>
                <td className="py-1.5 pr-2 align-middle">
                  <Select
                    value={dim.format ?? defaultFormatForType(dim.type)}
                    onValueChange={(value) =>
                      updatePatch(index, {
                        format: value as ReportDatasetValueFormat,
                      })
                    }
                  >
                    <SelectTrigger className="h-7">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {VALUE_FORMATS.map((option) => (
                        <SelectItem key={option.value} value={option.value}>
                          {option.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </td>
                <td className="py-1.5 text-right align-middle">
                  <Button
                    type="button"
                    size="icon"
                    variant="ghost"
                    className="h-7 w-7"
                    onClick={() => removeDimension(index)}
                    aria-label={`Remove ${dim.field}`}
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      {availableFields.length > 0 ? (
        <Select
          value=""
          onValueChange={(value) => {
            if (value) addDimension(value);
          }}
        >
          <SelectTrigger className="h-8 w-auto min-w-[160px]">
            <SelectValue placeholder="+ Add dimension" />
          </SelectTrigger>
          <SelectContent>
            {availableFields.map((field) => (
              <SelectItem key={field} value={field}>
                {humanizeFieldName(field)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      ) : null}
    </div>
  );
}

function MeasuresEditor({
  measures,
  schemaFields,
  onChange,
}: {
  measures: ReportDatasetDefinition['measures'];
  schemaFields: string[];
  onChange: (next: ReportDatasetDefinition['measures']) => void;
}) {
  function addMeasure() {
    onChange([
      ...measures,
      {
        id: makeMeasureId(),
        label: 'New measure',
        op: 'count',
        format: 'number',
      },
    ]);
  }

  function updatePatch(
    index: number,
    patch: Partial<ReportDatasetDefinition['measures'][number]>
  ) {
    onChange(
      measures.map((measure, i) =>
        i === index ? { ...measure, ...patch } : measure
      )
    );
  }

  function removeMeasure(index: number) {
    onChange(measures.filter((_, i) => i !== index));
  }

  return (
    <div className="grid gap-2">
      <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
        Measures
      </Label>
      {measures.length === 0 ? (
        <p className="text-xs text-muted-foreground">
          No measures yet. Add one below.
        </p>
      ) : (
        <div className="grid gap-2">
          {measures.map((measure, index) => {
            const opRequiresField =
              measure.op !== 'count' && measure.op !== 'expr';
            const isPercentile =
              measure.op === 'percentile_cont' ||
              measure.op === 'percentile_disc';
            const isExpr = measure.op === 'expr';
            return (
              <div
                key={`${measure.id}-${index}`}
                className="grid gap-2 rounded-md border bg-muted/10 p-2"
              >
                <div className="grid gap-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_minmax(0,1fr)_auto]">
                  <Input
                    value={measure.id}
                    placeholder="measure_id"
                    onChange={(event) =>
                      updatePatch(index, {
                        id: event.target.value || makeMeasureId(),
                      })
                    }
                    className="h-8 font-mono text-xs"
                  />
                  <Input
                    value={measure.label}
                    placeholder="Label"
                    onChange={(event) =>
                      updatePatch(index, { label: event.target.value })
                    }
                    className="h-8"
                  />
                  <Select
                    value={measure.op}
                    onValueChange={(value) =>
                      updatePatch(index, {
                        op: value as ReportAggregateFn,
                        // Clear inputs that don't apply to the new op.
                        field:
                          value === 'count' || value === 'expr'
                            ? undefined
                            : measure.field,
                      })
                    }
                  >
                    <SelectTrigger className="h-8">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {MEASURE_OPS.map((option) => (
                        <SelectItem key={option.value} value={option.value}>
                          {option.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                  <Button
                    type="button"
                    size="icon"
                    variant="ghost"
                    className="h-8 w-8"
                    onClick={() => removeMeasure(index)}
                    aria-label={`Remove ${measure.id}`}
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </Button>
                </div>
                <div className="grid gap-2 sm:grid-cols-3">
                  {opRequiresField ? (
                    <div className="grid gap-1">
                      <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                        Field
                      </Label>
                      <Select
                        value={measure.field ?? schemaFields[0] ?? ''}
                        onValueChange={(value) =>
                          updatePatch(index, { field: value })
                        }
                      >
                        <SelectTrigger className="h-7">
                          <SelectValue placeholder="Select field" />
                        </SelectTrigger>
                        <SelectContent>
                          {schemaFields.map((field) => (
                            <SelectItem key={field} value={field}>
                              {field}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </div>
                  ) : null}
                  <div className="grid gap-1">
                    <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                      Format
                    </Label>
                    <Select
                      value={measure.format}
                      onValueChange={(value) =>
                        updatePatch(index, {
                          format: value as ReportDatasetValueFormat,
                        })
                      }
                    >
                      <SelectTrigger className="h-7">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {VALUE_FORMATS.map((option) => (
                          <SelectItem key={option.value} value={option.value}>
                            {option.label}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                  {(measure.op === 'sum' ||
                    measure.op === 'count' ||
                    measure.op === 'avg' ||
                    measure.op === 'min' ||
                    measure.op === 'max') && measure.op !== 'count' ? null : null}
                  {measure.op === 'count' || opRequiresField ? (
                    <label className="flex items-end gap-2 text-xs">
                      <Checkbox
                        checked={Boolean(measure.distinct)}
                        onCheckedChange={(checked) =>
                          updatePatch(index, { distinct: Boolean(checked) })
                        }
                      />
                      <span>Distinct values only</span>
                    </label>
                  ) : null}
                </div>
                {isPercentile ? (
                  <div className="grid gap-1">
                    <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                      Percentile (0–1)
                    </Label>
                    <Input
                      type="number"
                      min={0}
                      max={1}
                      step={0.05}
                      value={
                        measure.percentile !== undefined
                          ? String(measure.percentile)
                          : '0.5'
                      }
                      onChange={(event) =>
                        updatePatch(index, {
                          percentile: Number(event.target.value),
                        })
                      }
                      className="h-7"
                    />
                  </div>
                ) : null}
                {isExpr ? (
                  <div className="grid gap-1">
                    <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                      Expression (JSON)
                    </Label>
                    <Input
                      value={
                        typeof measure.expression === 'string'
                          ? measure.expression
                          : measure.expression
                            ? JSON.stringify(measure.expression)
                            : ''
                      }
                      placeholder='{"op":"divide","args":[...]}'
                      onChange={(event) => {
                        const raw = event.target.value;
                        try {
                          const parsed = raw ? JSON.parse(raw) : undefined;
                          updatePatch(index, { expression: parsed });
                        } catch {
                          updatePatch(index, { expression: raw });
                        }
                      }}
                      className="h-7 font-mono text-xs"
                    />
                  </div>
                ) : null}
              </div>
            );
          })}
        </div>
      )}
      <Button
        type="button"
        size="sm"
        variant="outline"
        className="h-8 w-fit"
        onClick={addMeasure}
      >
        <Plus className="mr-1 h-3 w-3" />
        Add measure
      </Button>
    </div>
  );
}
