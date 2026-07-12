import { useCallback, useMemo, useState } from 'react';
import { useAuth } from 'react-oidc-context';
import { useQueryClient } from '@tanstack/react-query';
import { Check, ChevronDown, Plus, Search, X } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { Checkbox } from '@/shared/components/ui/checkbox';
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from '@/shared/components/ui/popover';
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from '@/shared/components/ui/command';
import { ReportDefinition, ReportFilterDefinition } from '../types';
import { getFilterDefaultValue, TIME_RANGE_PRESETS } from '../utils';
import {
  FieldControl,
  useResolvedOptions,
  type FormDefinition,
  type OptionResolver,
} from '@/shared/forms';
import { resolveReportFilterOptions } from '../queries';
import { queryKeys } from '@/shared/queries/query-keys';
import {
  controlValueToReportRange,
  reportFilterToFormField,
  reportRangeToControlValue,
} from '../form-adapters';

type ReportFilterBarProps = {
  reportId?: string;
  definition: ReportDefinition;
  values: Record<string, unknown>;
  onChange: (filterId: string, value: unknown) => void;
  /**
   * Block ids visible in the current view. When provided, filters whose
   * `appliesTo` does not reference any visible block are hidden from the bar.
   * Pass `null` to disable the heuristic (legacy behavior).
   */
  visibleBlockIds?: Set<string> | null;
};

type FilterOption = { value: unknown; label: string; count?: number };

export function ReportFilterBar({
  reportId,
  definition,
  values,
  onChange,
  visibleBlockIds = null,
}: ReportFilterBarProps) {
  const [activatedIds, setActivatedIds] = useState<Set<string>>(new Set());

  if (definition.filters.length === 0) return null;

  const visibleFilters = definition.filters.filter((filter) =>
    isFilterVisible(filter, visibleBlockIds)
  );
  if (visibleFilters.length === 0) return null;

  const searchFilter = visibleFilters.find(
    (filter) => filter.type === 'search'
  );
  const nonSearchFilters = visibleFilters.filter(
    (filter) => filter.type !== 'search'
  );

  const isFilterActive = (filter: ReportFilterDefinition) => {
    if (filter.type === 'search') return false;
    if (activatedIds.has(filter.id)) return true;
    const value = values[filter.id];
    if (isEmptyValue(value)) return false;
    const defaultValue = getFilterDefaultValue(filter);
    if (isSameValue(value, defaultValue)) return false;
    return true;
  };

  const activeFilters = nonSearchFilters.filter(isFilterActive);
  const inactiveFilters = nonSearchFilters.filter(
    (filter) => !isFilterActive(filter)
  );

  const handleRemove = (filter: ReportFilterDefinition) => {
    setActivatedIds((prev) => {
      const next = new Set(prev);
      next.delete(filter.id);
      return next;
    });
    onChange(filter.id, getFilterDefaultValue(filter));
  };

  const handleActivate = (filter: ReportFilterDefinition) => {
    setActivatedIds((prev) => {
      const next = new Set(prev);
      next.add(filter.id);
      return next;
    });
  };

  return (
    <div className="flex flex-wrap items-center gap-2">
      {activeFilters.map((filter) => (
        <FilterChip
          key={filter.id}
          reportId={reportId}
          filter={filter}
          value={values[filter.id]}
          allValues={values}
          onChange={(value) => onChange(filter.id, value)}
          onRemove={() => handleRemove(filter)}
        />
      ))}
      {inactiveFilters.length > 0 && (
        <AddFilterMenu
          filters={inactiveFilters}
          onSelect={handleActivate}
          hasActive={activeFilters.length > 0}
        />
      )}
      {searchFilter && (
        <div className="ml-auto">
          <SearchFilter
            filter={searchFilter}
            value={values[searchFilter.id]}
            onChange={(value) => onChange(searchFilter.id, value)}
          />
        </div>
      )}
    </div>
  );
}

function FilterChip({
  reportId,
  filter,
  value,
  allValues,
  onChange,
  onRemove,
}: {
  reportId?: string;
  filter: ReportFilterDefinition;
  value: unknown;
  allValues: Record<string, unknown>;
  onChange: (value: unknown) => void;
  onRemove: () => void;
}) {
  const [open, setOpen] = useState(false);
  const auth = useAuth();
  const queryClient = useQueryClient();
  const usesDynamicOptions = filter.options?.source === 'object_model';
  const optionDefinition = useMemo<FormDefinition>(
    () => {
      const field = reportFilterToFormField(filter);
      return {
        fields: {
          [filter.id]: {
            ...field,
            control: {
              ...field.control!,
              optionResolver:
                reportId && usesDynamicOptions && open
                  ? 'reports.filter-options'
                  : undefined,
              optionDependencies: Object.keys(allValues).sort(),
            },
          },
        },
      };
    },
    [allValues, filter, open, reportId, usesDynamicOptions]
  );
  const resolveOptions = useCallback<OptionResolver>(
    async ({ resolverKey, currentData, signal }) => {
      if (resolverKey !== 'reports.filter-options' || !reportId) return [];
      const request = {
        filters: currentData,
        limit: 200,
        timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
      };
      const response = await queryClient.fetchQuery({
        queryKey: queryKeys.reports.filterOptions(
          reportId,
          filter.id,
          request
        ),
        queryFn: () =>
          resolveReportFilterOptions(
            auth.user?.access_token ?? '',
            reportId,
            filter.id,
            request,
            signal
          ),
        staleTime: 30_000,
      });
      return response.options;
    },
    [auth.user?.access_token, filter.id, queryClient, reportId]
  );
  const resolvedOptions = useResolvedOptions(
    optionDefinition,
    allValues,
    resolveOptions
  );
  const dynamicOptions = resolvedOptions.options[filter.id] as
    | FilterOption[]
    | undefined;
  const isLoadingOptions = resolvedOptions.loading.has(filter.id);
  const options: FilterOption[] =
    dynamicOptions ?? filter.options?.values ?? [];
  const summary = describeFilterValue(filter, value, options);

  return (
    <div className="inline-flex h-8 items-center overflow-hidden rounded-full border bg-background text-sm shadow-sm">
      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <button
            type="button"
            className="flex h-full items-center gap-1.5 px-3 hover:bg-muted/40"
          >
            <span className="text-muted-foreground">{filter.label}:</span>
            <span className="font-medium">{summary}</span>
            <ChevronDown className="h-3.5 w-3.5 opacity-50" />
          </button>
        </PopoverTrigger>
        <PopoverContent className="w-72 p-0" align="start">
          {resolvedOptions.errors[filter.id] && (
            <p className="border-b px-3 py-2 text-xs text-destructive" role="alert">
              {resolvedOptions.errors[filter.id]}
            </p>
          )}
          <FilterEditor
            filter={filter}
            value={value}
            options={options}
            isLoadingOptions={isLoadingOptions}
            onChange={onChange}
          />
        </PopoverContent>
      </Popover>
      <button
        type="button"
        onClick={onRemove}
        aria-label={`Remove ${filter.label} filter`}
        className="flex h-full items-center border-l px-2 text-muted-foreground hover:bg-muted/40 hover:text-foreground"
      >
        <X className="h-3.5 w-3.5" />
      </button>
    </div>
  );
}

function AddFilterMenu({
  filters,
  onSelect,
  hasActive,
}: {
  filters: ReportFilterDefinition[];
  onSelect: (filter: ReportFilterDefinition) => void;
  hasActive: boolean;
}) {
  const [open, setOpen] = useState(false);
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-8 gap-1.5 rounded-full px-3 text-sm font-normal text-muted-foreground"
        >
          <Plus className="h-3.5 w-3.5" />
          {hasActive ? 'Add filter' : 'Filter'}
        </Button>
      </PopoverTrigger>
      <PopoverContent className="w-60 p-0" align="start">
        <Command>
          <CommandInput placeholder="Search filters..." />
          <CommandList>
            <CommandEmpty>No filters.</CommandEmpty>
            <CommandGroup>
              {filters.map((filter) => (
                <CommandItem
                  key={filter.id}
                  value={filter.label}
                  onSelect={() => {
                    onSelect(filter);
                    setOpen(false);
                  }}
                >
                  {filter.label}
                </CommandItem>
              ))}
            </CommandGroup>
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}

function SearchFilter({
  filter,
  value,
  onChange,
}: {
  filter: ReportFilterDefinition;
  value: unknown;
  onChange: (value: unknown) => void;
}) {
  return (
    <div className="relative w-72">
      <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
      <Input
        type="search"
        className="h-9 pl-9"
        value={String(value ?? '')}
        onChange={(event) => onChange(event.target.value)}
        placeholder={filter.label || 'Search'}
      />
    </div>
  );
}

function FilterEditor({
  filter,
  value,
  options,
  isLoadingOptions,
  onChange,
}: {
  filter: ReportFilterDefinition;
  value: unknown;
  options: FilterOption[];
  isLoadingOptions: boolean;
  onChange: (value: unknown) => void;
}) {
  const field = reportFilterToFormField(
    filter,
    filter.type === 'time_range' ? TIME_RANGE_PRESETS : options
  );
  const kind = field.control?.kind;

  if (filter.type === 'time_range') {
    return (
      <div className="p-3">
        <FieldControl
          id={`report-filter-${filter.id}`}
          field={field}
          value={value ?? 'last_30_days'}
          disabled={false}
          onChange={onChange}
        />
      </div>
    );
  }

  if (kind === 'number_range') {
    return (
      <div className="p-3">
        <FieldControl
          id={`report-filter-${filter.id}`}
          field={field}
          value={reportRangeToControlValue(value)}
          disabled={false}
          onChange={(next) => onChange(controlValueToReportRange(next))}
        />
      </div>
    );
  }

  if (filter.type === 'select' || filter.type === 'radio') {
    const selectedKey = optionKey(value);
    return (
      <Command
        filter={(itemValue, search) => {
          if (!search) return 1;
          return itemValue.toLowerCase().includes(search.toLowerCase()) ? 1 : 0;
        }}
      >
        <CommandInput placeholder={`Search ${filter.label.toLowerCase()}...`} />
        <CommandList>
          <CommandEmpty>
            {isLoadingOptions ? 'Loading...' : 'No options.'}
          </CommandEmpty>
          <CommandGroup>
            {options.map((option) => {
              const key = optionKey(option.value);
              const checked = key === selectedKey;
              return (
                <CommandItem
                  key={key}
                  value={`${option.label} ${key}`}
                  onSelect={() => onChange(option.value)}
                >
                  <span className="flex-1 truncate">
                    {formatOptionLabel(option.label, option.count)}
                  </span>
                  {checked && <Check className="h-4 w-4 opacity-70" />}
                </CommandItem>
              );
            })}
          </CommandGroup>
        </CommandList>
      </Command>
    );
  }

  if (filter.type === 'multi_select') {
    const selectedValues = Array.isArray(value) ? value : [];
    const selectedKeys = new Set(selectedValues.map(optionKey));
    const toggle = (option: FilterOption) => {
      const key = optionKey(option.value);
      const next = selectedKeys.has(key)
        ? selectedValues.filter((item) => optionKey(item) !== key)
        : [...selectedValues, option.value];
      onChange(next);
    };
    return (
      <Command
        filter={(itemValue, search) => {
          if (!search) return 1;
          return itemValue.toLowerCase().includes(search.toLowerCase()) ? 1 : 0;
        }}
      >
        <CommandInput placeholder={`Search ${filter.label.toLowerCase()}...`} />
        <CommandList>
          <CommandEmpty>
            {isLoadingOptions ? 'Loading...' : 'No options.'}
          </CommandEmpty>
          <CommandGroup>
            {options.map((option) => {
              const key = optionKey(option.value);
              const checked = selectedKeys.has(key);
              return (
                <CommandItem
                  key={key}
                  value={`${option.label} ${key}`}
                  onSelect={() => toggle(option)}
                >
                  <Checkbox checked={checked} className="pointer-events-none" />
                  <span className="flex-1 truncate">
                    {formatOptionLabel(option.label, option.count)}
                  </span>
                </CommandItem>
              );
            })}
          </CommandGroup>
        </CommandList>
      </Command>
    );
  }

  if (kind === 'toggle') {
    return (
      <div className="p-3">
        <FieldControl
          id={`report-filter-${filter.id}`}
          field={field}
          value={value}
          disabled={false}
          onChange={onChange}
        />
      </div>
    );
  }

  return (
    <div className="p-2">
      <FieldControl
        id={`report-filter-${filter.id}`}
        field={field}
        value={value}
        disabled={false}
        onChange={onChange}
      />
    </div>
  );
}

function isFilterVisible(
  filter: ReportFilterDefinition,
  visibleBlockIds: Set<string> | null
): boolean {
  if (visibleBlockIds === null) return true;
  const appliesTo = filter.appliesTo ?? [];
  if (appliesTo.length === 0) return false;
  return appliesTo.some(
    (target) => target.blockId && visibleBlockIds.has(target.blockId)
  );
}

function describeFilterValue(
  filter: ReportFilterDefinition,
  value: unknown,
  options: FilterOption[]
): string {
  if (filter.type === 'multi_select' && Array.isArray(value)) {
    if (value.length === 0) return 'Any';
    if (value.length === 1) return labelForValue(value[0], options);
    return `${value.length} selected`;
  }
  if (filter.type === 'time_range') {
    const preset = TIME_RANGE_PRESETS.find(
      (option) => option.value === String(value ?? '')
    );
    return preset?.label ?? 'Custom';
  }
  if (filter.type === 'checkbox') {
    return value ? 'Yes' : 'No';
  }
  if (filter.type === 'number_range') {
    if (!value || typeof value !== 'object' || Array.isArray(value)) {
      return 'Any';
    }
    const range = value as { min?: unknown; max?: unknown };
    if (range.min !== undefined && range.max !== undefined) {
      return `${range.min} - ${range.max}`;
    }
    if (range.min !== undefined) return `>= ${range.min}`;
    if (range.max !== undefined) return `<= ${range.max}`;
    return 'Any';
  }
  if (isEmptyValue(value)) return 'Any';
  return labelForValue(value, options);
}

function labelForValue(value: unknown, options: FilterOption[]): string {
  const key = optionKey(value);
  const match = options.find((option) => optionKey(option.value) === key);
  return match?.label ?? String(value);
}

function optionKey(value: unknown): string {
  if (value === null || value === undefined) return '__empty__';
  if (typeof value === 'string') return value;
  return JSON.stringify(value);
}

function formatOptionLabel(label: string, count?: number): string {
  if (typeof count !== 'number') return label;
  return `${label} (${new Intl.NumberFormat().format(count)})`;
}

function isEmptyValue(value: unknown): boolean {
  if (value === null || value === undefined) return true;
  if (typeof value === 'string') return value.trim().length === 0;
  if (Array.isArray(value)) return value.length === 0;
  if (typeof value === 'object') {
    const range = value as { min?: unknown; max?: unknown };
    if ('min' in range || 'max' in range) {
      return range.min === undefined && range.max === undefined;
    }
  }
  return false;
}

function isSameValue(a: unknown, b: unknown): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}
