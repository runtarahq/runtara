import { Plus, Trash2 } from 'lucide-react';
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
import { Textarea } from '@/shared/components/ui/textarea';
import { Schema } from '@/generated/RuntaraRuntimeApi';
import { ReportFilterType } from '../../../types';
import { TIME_RANGE_PRESETS } from '../../../utils';
import {
  WIZARD_FILTER_TARGET_ALL,
  WIZARD_FILTER_TARGET_NONE,
  WizardBlock,
  WizardFilter,
  WizardFilterOptionsSource,
} from '../wizardTypes';

interface ControlsStepProps {
  filters: WizardFilter[];
  blocks: WizardBlock[];
  schemas: Schema[];
  onChange: (next: WizardFilter[]) => void;
}

function fieldsOf(schemas: Schema[], schemaName: string | undefined): string[] {
  if (!schemaName) return [];
  return (
    schemas.find((schema) => schema.name === schemaName)?.columns.map(
      (column) => column.name
    ) ?? []
  );
}

const FILTER_TYPES: Array<{ value: ReportFilterType; label: string }> = [
  { value: 'select', label: 'Single select' },
  { value: 'multi_select', label: 'Multi select' },
  { value: 'radio', label: 'Radio' },
  { value: 'checkbox', label: 'Checkbox' },
  { value: 'time_range', label: 'Time range' },
  { value: 'number_range', label: 'Number range' },
  { value: 'text', label: 'Text' },
  { value: 'search', label: 'Search' },
];

function filterUsesOptions(type: ReportFilterType): boolean {
  return type === 'select' || type === 'multi_select' || type === 'radio';
}

export function ControlsStep({
  filters,
  blocks,
  schemas,
  onChange,
}: ControlsStepProps) {
  // A filter's field options depend on which block it targets. When targeting
  // "all compatible", we fall back to the union of fields across blocks with a
  // schema. When targeting a specific block, we use that block's schema.
  const fieldsByBlockId = Object.fromEntries(
    blocks.map((block) => [block.id, fieldsOf(schemas, block.schema)])
  );
  const allBlockFields = Array.from(
    new Set(blocks.flatMap((block) => fieldsOf(schemas, block.schema)))
  );
  const initialField = allBlockFields[0] ?? 'id';

  function addFilter() {
    const id = `filter_${Date.now().toString(36)}`;
    onChange([
      ...filters,
      {
        id,
        label: 'New filter',
        field: initialField,
        type: 'select',
        target: WIZARD_FILTER_TARGET_ALL,
        optionsSource: 'object_model',
        optionsField: initialField,
      },
    ]);
  }

  function updateFilter(index: number, patch: Partial<WizardFilter>) {
    onChange(
      filters.map((filter, currentIndex) =>
        currentIndex === index ? { ...filter, ...patch } : filter
      )
    );
  }

  function removeFilter(index: number) {
    onChange(filters.filter((_, currentIndex) => currentIndex !== index));
  }

  const filterableBlocks = blocks.filter((block) => block.type !== 'markdown');

  return (
    <div className="grid gap-3">
      <div className="rounded-md border-l-4 border-emerald-500 bg-emerald-50/60 px-3 py-2 text-sm text-emerald-900 dark:bg-emerald-950/30 dark:text-emerald-200">
        Filters render at the top of the report. Map each one to all compatible
        blocks or a specific block — anything left unconnected appears as a
        warning at the Review step.
      </div>

      <div className="flex items-center justify-between">
        <span className="text-sm text-muted-foreground">
          {filters.length === 0
            ? 'No filters yet.'
            : `${filters.length} filter${filters.length === 1 ? '' : 's'} configured.`}
        </span>
        <Button type="button" variant="outline" size="sm" onClick={addFilter}>
          <Plus className="mr-2 h-4 w-4" />
          Add filter
        </Button>
      </div>

      {filters.length === 0 ? (
        <div className="rounded-md border border-dashed bg-muted/20 p-6 text-center text-sm text-muted-foreground">
          Reports work without filters too. Add one to let viewers narrow down
          the data.
        </div>
      ) : (
        <div className="grid gap-3">
          {filters.map((filter, index) => {
            const fieldsForThisFilter =
              filter.target === WIZARD_FILTER_TARGET_ALL ||
              filter.target === WIZARD_FILTER_TARGET_NONE
                ? allBlockFields
                : fieldsByBlockId[filter.target] ?? allBlockFields;
            return (
              <FilterRow
                key={filter.id || index}
                filter={filter}
                schemaFields={fieldsForThisFilter}
                filterableBlocks={filterableBlocks}
                onChange={(patch) => updateFilter(index, patch)}
                onRemove={() => removeFilter(index)}
              />
            );
          })}
        </div>
      )}
    </div>
  );
}

function FilterRow({
  filter,
  schemaFields,
  filterableBlocks,
  onChange,
  onRemove,
}: {
  filter: WizardFilter;
  schemaFields: string[];
  filterableBlocks: WizardBlock[];
  onChange: (patch: Partial<WizardFilter>) => void;
  onRemove: () => void;
}) {
  const showOptions = filterUsesOptions(filter.type);
  return (
    <div className="grid gap-3 rounded-md border bg-background p-3">
      <div className="grid gap-3 lg:grid-cols-[minmax(0,1.2fr)_minmax(0,1fr)_minmax(0,1fr)_minmax(0,1.2fr)_auto]">
        <div className="grid gap-1.5">
          <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Viewer label
          </Label>
          <Input
            value={filter.label}
            onChange={(event) => onChange({ label: event.target.value })}
          />
        </div>
        <div className="grid gap-1.5">
          <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Field
          </Label>
          <Select
            value={filter.field || schemaFields[0]}
            onValueChange={(value) => onChange({ field: value })}
          >
            <SelectTrigger>
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
        <div className="grid gap-1.5">
          <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Control
          </Label>
          <Select
            value={filter.type}
            onValueChange={(value) =>
              onChange({
                type: value as ReportFilterType,
                defaultValue: undefined,
              })
            }
          >
            <SelectTrigger>
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
        <div className="grid gap-1.5">
          <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Applies to
          </Label>
          <Select
            value={filter.target}
            onValueChange={(value) => onChange({ target: value })}
          >
            <SelectTrigger>
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={WIZARD_FILTER_TARGET_ALL}>
                All compatible blocks
              </SelectItem>
              <SelectItem value={WIZARD_FILTER_TARGET_NONE}>
                Not connected
              </SelectItem>
              {filterableBlocks.map((block) => (
                <SelectItem key={block.id} value={block.id}>
                  {block.title || block.id}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          onClick={onRemove}
          aria-label="Remove filter"
          className="self-end"
        >
          <Trash2 className="h-4 w-4" />
        </Button>
      </div>

      {showOptions ? (
        <div className="grid gap-3 rounded-md border bg-muted/10 p-3 sm:grid-cols-[minmax(0,1fr)_minmax(0,2fr)]">
          <div className="grid gap-1.5">
            <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
              Options
            </Label>
            <Select
              value={filter.optionsSource ?? 'object_model'}
              onValueChange={(value) =>
                onChange({
                  optionsSource: value as WizardFilterOptionsSource,
                })
              }
            >
              <SelectTrigger>
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="object_model">
                  Pull from schema field
                </SelectItem>
                <SelectItem value="static">Static values</SelectItem>
              </SelectContent>
            </Select>
          </div>
          {(filter.optionsSource ?? 'object_model') === 'object_model' ? (
            <div className="grid gap-1.5">
              <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
                Schema field to look up
              </Label>
              <Select
                value={filter.optionsField || filter.field || schemaFields[0]}
                onValueChange={(value) =>
                  onChange({ optionsField: value })
                }
              >
                <SelectTrigger>
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
          ) : (
            <div className="grid gap-1.5">
              <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
                Static values (one per line — optional `value=Label`)
              </Label>
              <Textarea
                rows={3}
                value={filter.staticOptions ?? ''}
                onChange={(event) =>
                  onChange({ staticOptions: event.target.value })
                }
                placeholder={'open=Open\nclosed=Closed\npending=Pending'}
              />
            </div>
          )}
        </div>
      ) : null}

      <div className="grid gap-3 sm:grid-cols-3">
        <DefaultValueEditor filter={filter} onChange={onChange} />
        <label className="flex min-h-10 items-center gap-2 rounded-md border bg-background px-3 text-sm">
          <Checkbox
            checked={Boolean(filter.required)}
            onCheckedChange={(checked) =>
              onChange({ required: Boolean(checked) })
            }
          />
          Required
        </label>
        <label className="flex min-h-10 items-center gap-2 rounded-md border bg-background px-3 text-sm">
          <Checkbox
            checked={Boolean(filter.strictWhenReferenced)}
            onCheckedChange={(checked) =>
              onChange({ strictWhenReferenced: Boolean(checked) })
            }
          />
          Hide dependent blocks until set
        </label>
      </div>
    </div>
  );
}

function DefaultValueEditor({
  filter,
  onChange,
}: {
  filter: WizardFilter;
  onChange: (patch: Partial<WizardFilter>) => void;
}) {
  const label = (
    <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
      Default value
    </Label>
  );
  if (filter.type === 'checkbox') {
    return (
      <div className="grid gap-1.5">
        {label}
        <label className="flex min-h-10 items-center gap-2 rounded-md border bg-background px-3 text-sm">
          <Checkbox
            checked={Boolean(filter.defaultValue)}
            onCheckedChange={(checked) =>
              onChange({ defaultValue: Boolean(checked) })
            }
          />
          Checked by default
        </label>
      </div>
    );
  }
  if (filter.type === 'time_range') {
    return (
      <div className="grid gap-1.5">
        {label}
        <Select
          value={String(filter.defaultValue ?? 'last_30_days')}
          onValueChange={(value) => onChange({ defaultValue: value })}
        >
          <SelectTrigger>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {TIME_RANGE_PRESETS.map((preset) => (
              <SelectItem key={preset.value} value={preset.value}>
                {preset.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
    );
  }
  if (filter.type === 'multi_select') {
    const arr = Array.isArray(filter.defaultValue) ? filter.defaultValue : [];
    return (
      <div className="grid gap-1.5">
        {label}
        <Input
          value={arr.join(', ')}
          onChange={(event) =>
            onChange({
              defaultValue: event.target.value
                .split(',')
                .map((value) => value.trim())
                .filter(Boolean),
            })
          }
          placeholder="open, pending"
        />
      </div>
    );
  }
  return (
    <div className="grid gap-1.5">
      {label}
      <Input
        value={
          typeof filter.defaultValue === 'string' ||
          typeof filter.defaultValue === 'number'
            ? String(filter.defaultValue)
            : ''
        }
        onChange={(event) =>
          onChange({
            defaultValue: event.target.value ? event.target.value : undefined,
          })
        }
        placeholder="(none)"
      />
    </div>
  );
}
