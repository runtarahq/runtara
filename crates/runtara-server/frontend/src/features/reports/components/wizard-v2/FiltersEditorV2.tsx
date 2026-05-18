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
  ReportDefinition,
  ReportFilterDefinition,
  ReportFilterOptionsConfig,
  ReportFilterTarget,
  ReportFilterType,
} from '../../types';

type StaticOption = NonNullable<ReportFilterOptionsConfig['values']>[number];

const FILTER_TYPES: Array<{ value: ReportFilterType; label: string }> = [
  { value: 'text', label: 'Text' },
  { value: 'number_range', label: 'Number range' },
  { value: 'select', label: 'Single select' },
  { value: 'multi_select', label: 'Multi-select' },
  { value: 'time_range', label: 'Time range' },
  { value: 'radio', label: 'Radio' },
  { value: 'checkbox', label: 'Checkbox' },
  { value: 'search', label: 'Search' },
];

const OPTIONS_SOURCES = [
  { value: 'static', label: 'Static list' },
  { value: 'object_model', label: 'Object Model lookup' },
] as const;

const OPS = [
  'eq',
  'ne',
  'gt',
  'gte',
  'lt',
  'lte',
  'in',
  'not_in',
  'contains',
  'between',
];

interface FiltersEditorV2Props {
  definition: ReportDefinition;
  schemas: Schema[];
  onChange: (definition: ReportDefinition) => void;
}

function newFilter(): ReportFilterDefinition {
  return {
    id: `filter_${Math.random().toString(36).slice(2, 7)}`,
    label: 'New filter',
    type: 'text',
  };
}

export function FiltersEditorV2({
  definition,
  schemas,
  onChange,
}: FiltersEditorV2Props) {
  const filters = definition.filters ?? [];

  const updateFilters = (next: ReportFilterDefinition[]) =>
    onChange({ ...definition, filters: next });

  const updateFilter = (
    id: string,
    updater: (filter: ReportFilterDefinition) => ReportFilterDefinition
  ) => updateFilters(filters.map((f) => (f.id === id ? updater(f) : f)));

  const otherFilterIds = filters.map((f) => f.id);
  const blockIds = definition.blocks.map((b) => b.id);

  return (
    <div className="grid gap-3">
      {filters.length === 0 ? (
        <p className="text-sm text-muted-foreground">
          No filters yet. Add one below to expose viewer-facing controls.
        </p>
      ) : (
        <div className="grid gap-3">
          {filters.map((filter) => (
            <Card key={filter.id}>
              <CardHeader className="flex flex-row items-center justify-between gap-2 space-y-0 py-3">
                <CardTitle className="text-sm">{filter.label}</CardTitle>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="h-7 w-7 text-destructive"
                  onClick={() =>
                    updateFilters(filters.filter((f) => f.id !== filter.id))
                  }
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              </CardHeader>
              <CardContent className="grid gap-3 pt-0">
                <div className="grid grid-cols-3 gap-3">
                  <div className="grid gap-1.5">
                    <Label className="text-xs">ID</Label>
                    <Input
                      value={filter.id}
                      onChange={(event) =>
                        updateFilter(filter.id, (f) => ({
                          ...f,
                          id: event.target.value,
                        }))
                      }
                    />
                  </div>
                  <div className="grid gap-1.5">
                    <Label className="text-xs">Label</Label>
                    <Input
                      value={filter.label}
                      onChange={(event) =>
                        updateFilter(filter.id, (f) => ({
                          ...f,
                          label: event.target.value,
                        }))
                      }
                    />
                  </div>
                  <div className="grid gap-1.5">
                    <Label className="text-xs">Type</Label>
                    <Select
                      value={filter.type}
                      onValueChange={(value) =>
                        updateFilter(filter.id, (f) => ({
                          ...f,
                          type: value as ReportFilterType,
                        }))
                      }
                    >
                      <SelectTrigger className="h-9">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {FILTER_TYPES.map((option) => (
                          <SelectItem key={option.value} value={option.value}>
                            {option.label}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                </div>

                <div className="flex flex-wrap items-center gap-3 text-xs">
                  <label className="inline-flex items-center gap-1.5">
                    <input
                      type="checkbox"
                      checked={Boolean(filter.required)}
                      onChange={(event) =>
                        updateFilter(filter.id, (f) => ({
                          ...f,
                          required: event.target.checked,
                        }))
                      }
                    />
                    Required
                  </label>
                  <label className="inline-flex items-center gap-1.5">
                    <input
                      type="checkbox"
                      checked={Boolean(filter.strictWhenReferenced)}
                      onChange={(event) =>
                        updateFilter(filter.id, (f) => ({
                          ...f,
                          strictWhenReferenced: event.target.checked,
                        }))
                      }
                    />
                    Strict when referenced
                  </label>
                </div>

                <FilterOptionsEditor
                  filter={filter}
                  schemas={schemas}
                  otherFilterIds={otherFilterIds.filter(
                    (id) => id !== filter.id
                  )}
                  onChange={(updated) =>
                    updateFilter(filter.id, () => updated)
                  }
                />

                <AppliesToEditor
                  filter={filter}
                  blockIds={blockIds}
                  onChange={(updated) =>
                    updateFilter(filter.id, () => updated)
                  }
                />
              </CardContent>
            </Card>
          ))}
        </div>
      )}
      <div>
        <Button
          type="button"
          variant="outline"
          onClick={() => updateFilters([...filters, newFilter()])}
        >
          <Plus className="mr-1 h-3.5 w-3.5" /> Add filter
        </Button>
      </div>
    </div>
  );
}

interface FilterOptionsEditorProps {
  filter: ReportFilterDefinition;
  schemas: Schema[];
  otherFilterIds: string[];
  onChange: (filter: ReportFilterDefinition) => void;
}

function FilterOptionsEditor({
  filter,
  schemas,
  otherFilterIds,
  onChange,
}: FilterOptionsEditorProps) {
  const options: ReportFilterOptionsConfig = (filter.options ?? {}) as
    | ReportFilterOptionsConfig
    | Record<string, never>;
  const source = options.source ?? 'static';
  const schema = schemas.find((s) => s.name === options.schema);
  const fields = schema?.columns.map((c) => c.name) ?? [];

  const updateOptions = (next: ReportFilterOptionsConfig) =>
    onChange({ ...filter, options: next });

  return (
    <div className="grid gap-2 rounded border p-2">
      <Label className="text-xs">Options source</Label>
      <Select
        value={source}
        onValueChange={(value) =>
          updateOptions({
            ...options,
            source: value as ReportFilterOptionsConfig['source'],
          })
        }
      >
        <SelectTrigger className="h-9">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {OPTIONS_SOURCES.map((opt) => (
            <SelectItem key={opt.value} value={opt.value}>
              {opt.label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>

      {source === 'static' ? (
        <StaticOptionsEditor
          values={options.values ?? []}
          onChange={(values) => updateOptions({ ...options, values })}
        />
      ) : (
        <ObjectModelOptionsEditor
          options={options}
          schemas={schemas}
          fields={fields}
          otherFilterIds={otherFilterIds}
          onChange={updateOptions}
        />
      )}
    </div>
  );
}

interface StaticOptionsEditorProps {
  values: StaticOption[];
  onChange: (values: StaticOption[]) => void;
}

function StaticOptionsEditor({
  values,
  onChange,
}: StaticOptionsEditorProps) {
  return (
    <div className="grid gap-1.5">
      <div className="flex items-center justify-between">
        <Label className="text-xs">Values</Label>
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-7"
          onClick={() => onChange([...values, { label: '', value: '' }])}
        >
          <Plus className="mr-1 h-3 w-3" /> Add value
        </Button>
      </div>
      {values.length === 0 ? (
        <p className="text-xs text-muted-foreground">No values yet.</p>
      ) : (
        <div className="grid gap-1.5">
          {values.map((option, index) => (
            <div
              key={index}
              className="grid grid-cols-[1fr_1fr_minmax(0,auto)] gap-2"
            >
              <Input
                value={String(option.value ?? '')}
                className="h-8 text-xs"
                placeholder="Value"
                onChange={(event) =>
                  onChange(
                    values.map((v, i) =>
                      i === index ? { ...v, value: event.target.value } : v
                    )
                  )
                }
              />
              <Input
                value={option.label}
                className="h-8 text-xs"
                placeholder="Label"
                onChange={(event) =>
                  onChange(
                    values.map((v, i) =>
                      i === index ? { ...v, label: event.target.value } : v
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
                  onChange(values.filter((_, i) => i !== index))
                }
              >
                <Trash2 className="h-3.5 w-3.5" />
              </Button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

interface ObjectModelOptionsEditorProps {
  options: ReportFilterOptionsConfig;
  schemas: Schema[];
  fields: string[];
  otherFilterIds: string[];
  onChange: (options: ReportFilterOptionsConfig) => void;
}

function ObjectModelOptionsEditor({
  options,
  schemas,
  fields,
  otherFilterIds,
  onChange,
}: ObjectModelOptionsEditorProps) {
  const filterMappings = options.filterMappings ?? [];
  const dependsOn = options.dependsOn ?? [];

  return (
    <div className="grid gap-2">
      <div className="grid grid-cols-2 gap-2">
        <div className="grid gap-1.5">
          <Label className="text-xs">Schema</Label>
          <Select
            value={options.schema || ''}
            onValueChange={(value) => onChange({ ...options, schema: value })}
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
        <div className="grid gap-1.5">
          <Label className="text-xs">Connection ID (optional)</Label>
          <Input
            value={options.connectionId ?? ''}
            placeholder="auto"
            onChange={(event) =>
              onChange({
                ...options,
                connectionId: event.target.value || undefined,
              })
            }
          />
        </div>
      </div>

      <div className="grid grid-cols-2 gap-2">
        <div className="grid gap-1.5">
          <Label className="text-xs">Value field</Label>
          <Select
            value={options.valueField || options.field || ''}
            onValueChange={(value) =>
              onChange({ ...options, valueField: value })
            }
          >
            <SelectTrigger className="h-9">
              <SelectValue placeholder="Pick a field" />
            </SelectTrigger>
            <SelectContent>
              {fields.map((f) => (
                <SelectItem key={f} value={f}>
                  {f}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="grid gap-1.5">
          <Label className="text-xs">Label field</Label>
          <Select
            value={
              options.labelField || options.valueField || options.field || ''
            }
            onValueChange={(value) =>
              onChange({ ...options, labelField: value })
            }
          >
            <SelectTrigger className="h-9">
              <SelectValue placeholder="Pick a field" />
            </SelectTrigger>
            <SelectContent>
              {fields.map((f) => (
                <SelectItem key={f} value={f}>
                  {f}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      </div>

      <label className="flex items-center gap-1.5 text-xs">
        <input
          type="checkbox"
          checked={Boolean(options.search)}
          onChange={(event) =>
            onChange({ ...options, search: event.target.checked })
          }
        />
        Server-side search
      </label>

      <div className="grid gap-1.5">
        <div className="flex items-center justify-between">
          <Label className="text-xs">Depends on</Label>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="h-7"
            onClick={() =>
              onChange({ ...options, dependsOn: [...dependsOn, ''] })
            }
            disabled={otherFilterIds.length === 0}
          >
            <Plus className="mr-1 h-3 w-3" /> Add filter
          </Button>
        </div>
        {dependsOn.length === 0 ? (
          <p className="text-xs text-muted-foreground">
            Other filters whose value gates this filter's options.
          </p>
        ) : (
          <div className="grid gap-1.5">
            {dependsOn.map((depId, index) => (
              <div
                key={index}
                className="grid grid-cols-[1fr_minmax(0,auto)] gap-2"
              >
                <Select
                  value={depId}
                  onValueChange={(value) =>
                    onChange({
                      ...options,
                      dependsOn: dependsOn.map((id, i) =>
                        i === index ? value : id
                      ),
                    })
                  }
                >
                  <SelectTrigger className="h-8 text-xs">
                    <SelectValue placeholder="Filter ID" />
                  </SelectTrigger>
                  <SelectContent>
                    {depId && !otherFilterIds.includes(depId) ? (
                      <SelectItem disabled value={depId}>
                        {depId}
                      </SelectItem>
                    ) : null}
                    {otherFilterIds.map((id) => (
                      <SelectItem key={id} value={id}>
                        {id}
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
                    onChange({
                      ...options,
                      dependsOn: dependsOn.filter((_, i) => i !== index),
                    })
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
          <Label className="text-xs">Filter mappings</Label>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="h-7"
            onClick={() =>
              onChange({
                ...options,
                filterMappings: [
                  ...filterMappings,
                  { filterId: '', field: '' },
                ],
              })
            }
          >
            <Plus className="mr-1 h-3 w-3" /> Add mapping
          </Button>
        </div>
        {filterMappings.length === 0 ? (
          <p className="text-xs text-muted-foreground">
            Map dependent filters' values into lookup fields.
          </p>
        ) : (
          <div className="grid gap-1.5">
            {filterMappings.map((mapping, index) => (
              <div
                key={index}
                className="grid grid-cols-[1fr_1fr_100px_minmax(0,auto)] gap-2"
              >
                <Select
                  value={mapping.filterId || ''}
                  onValueChange={(value) =>
                    onChange({
                      ...options,
                      filterMappings: filterMappings.map((m, i) =>
                        i === index ? { ...m, filterId: value } : m
                      ),
                    })
                  }
                >
                  <SelectTrigger className="h-8 text-xs">
                    <SelectValue placeholder="Filter" />
                  </SelectTrigger>
                  <SelectContent>
                    {otherFilterIds.map((id) => (
                      <SelectItem key={id} value={id}>
                        {id}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <Select
                  value={mapping.field || ''}
                  onValueChange={(value) =>
                    onChange({
                      ...options,
                      filterMappings: filterMappings.map((m, i) =>
                        i === index ? { ...m, field: value } : m
                      ),
                    })
                  }
                >
                  <SelectTrigger className="h-8 text-xs">
                    <SelectValue placeholder="Field" />
                  </SelectTrigger>
                  <SelectContent>
                    {fields.map((f) => (
                      <SelectItem key={f} value={f}>
                        {f}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <Select
                  value={mapping.op || 'eq'}
                  onValueChange={(value) =>
                    onChange({
                      ...options,
                      filterMappings: filterMappings.map((m, i) =>
                        i === index ? { ...m, op: value } : m
                      ),
                    })
                  }
                >
                  <SelectTrigger className="h-8 text-xs">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {OPS.map((op) => (
                      <SelectItem key={op} value={op}>
                        {op}
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
                    onChange({
                      ...options,
                      filterMappings: filterMappings.filter(
                        (_, i) => i !== index
                      ),
                    })
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

interface AppliesToEditorProps {
  filter: ReportFilterDefinition;
  blockIds: string[];
  onChange: (filter: ReportFilterDefinition) => void;
}

function AppliesToEditor({
  filter,
  blockIds,
  onChange,
}: AppliesToEditorProps) {
  const appliesTo = filter.appliesTo ?? [];

  return (
    <div className="grid gap-1.5 rounded border p-2">
      <div className="flex items-center justify-between">
        <Label className="text-xs">Applies to</Label>
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-7"
          onClick={() =>
            onChange({
              ...filter,
              appliesTo: [
                ...appliesTo,
                { blockId: blockIds[0] ?? null, field: '', op: 'eq' },
              ],
            })
          }
        >
          <Plus className="mr-1 h-3 w-3" /> Add target
        </Button>
      </div>
      {appliesTo.length === 0 ? (
        <p className="text-xs text-muted-foreground">
          Empty applies-to means the filter targets all blocks via their
          source's condition.
        </p>
      ) : (
        <div className="grid gap-1.5">
          {appliesTo.map((target: ReportFilterTarget, index: number) => (
            <div
              key={index}
              className="grid grid-cols-[1fr_1fr_100px_minmax(0,auto)] gap-2"
            >
              <Select
                value={target.blockId ?? ''}
                onValueChange={(value) =>
                  onChange({
                    ...filter,
                    appliesTo: appliesTo.map((t, i) =>
                      i === index
                        ? { ...t, blockId: value || null }
                        : t
                    ),
                  })
                }
              >
                <SelectTrigger className="h-8 text-xs">
                  <SelectValue placeholder="Block" />
                </SelectTrigger>
                <SelectContent>
                  {blockIds.map((id) => (
                    <SelectItem key={id} value={id}>
                      {id}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              <Input
                value={target.field}
                className="h-8 text-xs"
                placeholder="Field"
                onChange={(event) =>
                  onChange({
                    ...filter,
                    appliesTo: appliesTo.map((t, i) =>
                      i === index ? { ...t, field: event.target.value } : t
                    ),
                  })
                }
              />
              <Select
                value={target.op ?? 'eq'}
                onValueChange={(value) =>
                  onChange({
                    ...filter,
                    appliesTo: appliesTo.map((t, i) =>
                      i === index ? { ...t, op: value } : t
                    ),
                  })
                }
              >
                <SelectTrigger className="h-8 text-xs">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {OPS.map((op) => (
                    <SelectItem key={op} value={op}>
                      {op}
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
                  onChange({
                    ...filter,
                    appliesTo: appliesTo.filter((_, i) => i !== index),
                  })
                }
              >
                <Trash2 className="h-3.5 w-3.5" />
              </Button>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
