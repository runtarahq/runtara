import { Plus, Trash2 } from 'lucide-react';
import { Badge } from '@/shared/components/ui/badge';
import { Button } from '@/shared/components/ui/button';
import { Checkbox } from '@/shared/components/ui/checkbox';
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
  ReportBlockDatasetQuery,
  ReportChartKind,
  ReportDatasetDefinition,
  ReportDatasetFilterRequest,
} from '../../../types';
import {
  createDefaultDatasetBlockQuery,
  datasetFieldLabel,
  datasetQueryOutputFields,
} from '../../../datasetBlocks';
import { WizardBlock } from '../wizardTypes';

interface BlockDatasetQueryEditorProps {
  block: WizardBlock;
  datasets: ReportDatasetDefinition[];
  onChange: (patch: Partial<WizardBlock>) => void;
}

const NONE_VALUE = '__none__';

const OPERATOR_OPTIONS: Array<{ value: string; label: string }> = [
  { value: 'eq', label: 'equals' },
  { value: 'neq', label: 'not equals' },
  { value: 'gt', label: 'greater than' },
  { value: 'gte', label: 'greater than or equal' },
  { value: 'lt', label: 'less than' },
  { value: 'lte', label: 'less than or equal' },
  { value: 'in', label: 'in' },
  { value: 'not_in', label: 'not in' },
  { value: 'between', label: 'between' },
  { value: 'contains', label: 'contains' },
  { value: 'starts_with', label: 'starts with' },
];

const CHART_KINDS: Array<{ value: ReportChartKind; label: string }> = [
  { value: 'bar', label: 'Bar' },
  { value: 'line', label: 'Line' },
  { value: 'area', label: 'Area' },
  { value: 'pie', label: 'Pie' },
  { value: 'donut', label: 'Donut' },
];

function formatFilterValue(value: unknown, op?: string): string {
  if (value === null || value === undefined) return '';
  if (Array.isArray(value)) return value.map((entry) => String(entry)).join(', ');
  if (typeof value === 'object') {
    const record = value as { min?: unknown; max?: unknown };
    if (op === 'between' && (record.min !== undefined || record.max !== undefined)) {
      return `${record.min ?? ''}..${record.max ?? ''}`;
    }
    return JSON.stringify(value);
  }
  return String(value);
}

function parseFilterValue(value: string, op?: string): unknown {
  if (op === 'between') {
    const [min, max] = value.split('..').map((part) => part.trim());
    return {
      min: parseScalar(min ?? ''),
      max: parseScalar(max ?? ''),
    };
  }
  if (op === 'in' || op === 'not_in') {
    return value
      .split(',')
      .map((part) => parseScalar(part.trim()))
      .filter((entry) => entry !== '');
  }
  return parseScalar(value);
}

function parseScalar(value: string): unknown {
  if (value === '') return value;
  if (value === 'true') return true;
  if (value === 'false') return false;
  const asNumber = Number(value);
  if (!Number.isNaN(asNumber) && /^-?\d+(\.\d+)?$/.test(value)) return asNumber;
  return value;
}

export function BlockDatasetQueryEditor({
  block,
  datasets,
  onChange,
}: BlockDatasetQueryEditorProps) {
  const query = block.dataset;
  const dataset = query
    ? datasets.find((candidate) => candidate.id === query.id)
    : undefined;

  function switchDataset(id: string) {
    if (id === NONE_VALUE) return;
    const next = datasets.find((d) => d.id === id);
    if (!next) return;
    const seeded = createDefaultDatasetBlockQuery(next, query);
    onChange({ dataset: seeded, fields: [], fieldConfigs: undefined });
  }

  if (datasets.length === 0) {
    return (
      <div className="rounded-md border border-dashed bg-muted/10 p-3 text-xs text-muted-foreground">
        Add a dataset in the Datasets section before using one here.
      </div>
    );
  }

  if (!query) return null;

  if (!dataset) {
    return (
      <div className="grid gap-2">
        <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
          Dataset
        </Label>
        <div className="rounded-md border border-destructive/30 bg-destructive/5 p-3 text-xs text-destructive">
          This block references missing dataset "{query.id}". Pick a dataset
          below or remove the dataset reference from the block.
        </div>
        <Select value={query.id} onValueChange={switchDataset}>
          <SelectTrigger>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={query.id} disabled>
              Missing: {query.id}
            </SelectItem>
            {datasets.map((d) => (
              <SelectItem key={d.id} value={d.id}>
                {d.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
    );
  }

  const dimensions = query.dimensions ?? [];
  const measures = query.measures ?? [];
  const selectedDimensions = new Set(dimensions);
  const selectedMeasures = new Set(measures);
  const outputFields = datasetQueryOutputFields(query);
  const sort = query.orderBy?.[0];
  const datasetFilters = query.datasetFilters ?? [];
  const datasetFilterFields = dataset.dimensions.map((d) => d.field);

  const updateQuery = (next: ReportBlockDatasetQuery) => {
    onChange({ dataset: next });
  };

  const updateDatasetFilter = (
    index: number,
    patch: Partial<ReportDatasetFilterRequest>
  ) => {
    updateQuery({
      ...query,
      datasetFilters: datasetFilters.map((filter, i) =>
        i === index ? { ...filter, ...patch } : filter
      ),
    });
  };

  const addDatasetFilter = () => {
    updateQuery({
      ...query,
      datasetFilters: [
        ...datasetFilters,
        {
          field: datasetFilterFields[0] ?? outputFields[0] ?? '',
          op: 'eq',
          value: '',
        },
      ],
    });
  };

  return (
    <div className="grid gap-3">
      <div className="grid gap-1.5">
        <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
          Dataset
        </Label>
        <Select value={query.id} onValueChange={switchDataset}>
          <SelectTrigger>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {datasets.map((d) => (
              <SelectItem key={d.id} value={d.id}>
                {d.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      <div className="grid gap-3 sm:grid-cols-2">
        <div className="grid gap-2">
          <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Dimensions
          </Label>
          {dataset.dimensions.length === 0 ? (
            <p className="text-xs text-muted-foreground">
              This dataset has no dimensions.
            </p>
          ) : (
            <div className="grid gap-1">
              {dataset.dimensions.map((dimension) => (
                <label
                  key={dimension.field}
                  className="flex items-center gap-2 rounded-md border bg-background px-2 py-1.5 text-sm"
                >
                  <Checkbox
                    checked={selectedDimensions.has(dimension.field)}
                    onCheckedChange={(checked) => {
                      const next = checked
                        ? [...dimensions, dimension.field]
                        : dimensions.filter((f) => f !== dimension.field);
                      updateQuery({
                        ...query,
                        dimensions: next,
                        orderBy: (query.orderBy ?? []).filter((item) =>
                          [...next, ...measures].includes(item.field)
                        ),
                      });
                    }}
                  />
                  <span className="truncate">{dimension.label}</span>
                </label>
              ))}
            </div>
          )}
        </div>

        <div className="grid gap-2">
          <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Measures
          </Label>
          {dataset.measures.length === 0 ? (
            <p className="text-xs text-muted-foreground">
              This dataset has no measures.
            </p>
          ) : (
            <div className="grid gap-1">
              {dataset.measures.map((measure) => (
                <label
                  key={measure.id}
                  className="flex items-center gap-2 rounded-md border bg-background px-2 py-1.5 text-sm"
                >
                  <Checkbox
                    checked={selectedMeasures.has(measure.id)}
                    onCheckedChange={(checked) => {
                      const next = checked
                        ? [...measures, measure.id]
                        : measures.filter((id) => id !== measure.id);
                      updateQuery({
                        ...query,
                        measures: next,
                        orderBy: (query.orderBy ?? []).filter((item) =>
                          [...dimensions, ...next].includes(item.field)
                        ),
                      });
                    }}
                  />
                  <span className="truncate">{measure.label}</span>
                </label>
              ))}
            </div>
          )}
        </div>
      </div>

      <div className="grid gap-2 sm:grid-cols-3">
        <div className="grid gap-1.5">
          <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Sort by
          </Label>
          <Select
            value={sort?.field ?? NONE_VALUE}
            onValueChange={(field) =>
              updateQuery({
                ...query,
                orderBy:
                  field === NONE_VALUE
                    ? []
                    : [{ field, direction: sort?.direction ?? 'desc' }],
              })
            }
          >
            <SelectTrigger>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={NONE_VALUE}>No explicit sort</SelectItem>
              {outputFields.map((field) => (
                <SelectItem key={field} value={field}>
                  {datasetFieldLabel(dataset, field)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="grid gap-1.5">
          <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Direction
          </Label>
          <Select
            value={sort?.direction ?? 'desc'}
            disabled={!sort}
            onValueChange={(direction) =>
              updateQuery({
                ...query,
                orderBy: sort ? [{ ...sort, direction }] : [],
              })
            }
          >
            <SelectTrigger>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="desc">Descending</SelectItem>
              <SelectItem value="asc">Ascending</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <div className="grid gap-1.5">
          <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Limit
          </Label>
          <Input
            type="number"
            min={1}
            value={String(query.limit ?? 100)}
            onChange={(event) =>
              updateQuery({
                ...query,
                limit: Math.max(1, Number(event.target.value) || 100),
              })
            }
          />
        </div>
      </div>

      {block.type === 'chart' ? (
        <div className="grid gap-2 sm:grid-cols-2">
          <div className="grid gap-1.5">
            <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
              Chart style
            </Label>
            <Select
              value={block.chartKind ?? 'bar'}
              onValueChange={(value) =>
                onChange({ chartKind: value as ReportChartKind })
              }
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {CHART_KINDS.map((option) => (
                  <SelectItem key={option.value} value={option.value}>
                    {option.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className="grid gap-1.5">
            <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
              X-axis
            </Label>
            <Select
              value={block.chartGroupBy ?? dimensions[0] ?? ''}
              onValueChange={(value) => onChange({ chartGroupBy: value })}
            >
              <SelectTrigger>
                <SelectValue placeholder="Select field" />
              </SelectTrigger>
              <SelectContent>
                {outputFields.map((field) => (
                  <SelectItem key={field} value={field}>
                    {datasetFieldLabel(dataset, field)}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        </div>
      ) : null}

      <div className="grid gap-2 rounded-md border bg-muted/10 p-2">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex items-center gap-2">
            <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
              Block-fixed dataset filters
            </Label>
            <Badge variant="secondary">{datasetFilters.length}</Badge>
          </div>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="h-7"
            disabled={datasetFilterFields.length === 0}
            onClick={addDatasetFilter}
          >
            <Plus className="mr-1 h-3 w-3" />
            Add filter
          </Button>
        </div>
        {datasetFilters.length === 0 ? (
          <p className="text-xs text-muted-foreground">
            Optional. Use these for constraints that should always apply to
            this block before rendering.
          </p>
        ) : (
          <div className="grid gap-2">
            {datasetFilters.map((filter, index) => (
              <div
                key={`dataset-filter-${index}-${filter.field}`}
                className="grid gap-2 rounded-md border bg-background p-2 sm:grid-cols-[minmax(0,1fr)_10rem_minmax(0,1fr)_auto]"
              >
                <Select
                  value={filter.field || NONE_VALUE}
                  onValueChange={(field) =>
                    updateDatasetFilter(index, { field })
                  }
                >
                  <SelectTrigger className="h-8">
                    <SelectValue placeholder="Field" />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value={NONE_VALUE} disabled>
                      Select field
                    </SelectItem>
                    {datasetFilterFields.map((field) => (
                      <SelectItem key={field} value={field}>
                        {datasetFieldLabel(dataset, field)}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <Select
                  value={filter.op ?? 'eq'}
                  onValueChange={(op) => updateDatasetFilter(index, { op })}
                >
                  <SelectTrigger className="h-8">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {OPERATOR_OPTIONS.map((option) => (
                      <SelectItem key={option.value} value={option.value}>
                        {option.label}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                <Input
                  value={formatFilterValue(filter.value, filter.op)}
                  placeholder={
                    filter.op === 'between'
                      ? '10..20'
                      : filter.op === 'in' || filter.op === 'not_in'
                        ? 'open, pending'
                        : 'Value'
                  }
                  onChange={(event) =>
                    updateDatasetFilter(index, {
                      value: parseFilterValue(event.target.value, filter.op),
                    })
                  }
                  className="h-8"
                />
                <Button
                  type="button"
                  size="icon"
                  variant="ghost"
                  className="h-8 w-8"
                  onClick={() =>
                    updateQuery({
                      ...query,
                      datasetFilters: datasetFilters.filter(
                        (_, i) => i !== index
                      ),
                    })
                  }
                  aria-label="Remove dataset filter"
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
