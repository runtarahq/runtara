import { useMemo } from 'react';
import { X } from 'lucide-react';
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
import { Checkbox } from '@/shared/components/ui/checkbox';
import { ReportDefinition, ReportFilterDefinition } from '../types';
import { useReportFilterOptions } from '../hooks/useReports';
import { getFilterDefaultValue, TIME_RANGE_PRESETS } from '../utils';

type ReportFilterBarProps = {
  reportId?: string;
  definition: ReportDefinition;
  values: Record<string, unknown>;
  onChange: (filterId: string, value: unknown) => void;
  showChips?: boolean;
};

export function ReportFilterBar({
  reportId,
  definition,
  values,
  onChange,
  showChips = true,
}: ReportFilterBarProps) {
  if (definition.filters.length === 0) {
    return null;
  }

  const activeFilters = definition.filters.filter(
    (filter) => !isFilterDefault(filter, values[filter.id])
  );
  const chipFilters = activeFilters.filter(
    (filter) => filter.type !== 'multi_select' && filter.type !== 'checkbox'
  );

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-end gap-3">
        {definition.filters.map((filter) => (
          <FilterControl
            key={filter.id}
            reportId={reportId}
            filter={filter}
            value={values[filter.id]}
            allValues={values}
            onChange={(value) => onChange(filter.id, value)}
          />
        ))}
      </div>
      {showChips && chipFilters.length > 0 && (
        <div className="report-print-hidden flex flex-wrap items-center gap-2">
          {chipFilters.map((filter) => (
            <Button
              key={filter.id}
              type="button"
              variant="outline"
              size="sm"
              className="h-7 rounded-full px-3 text-xs"
              onClick={() => onChange(filter.id, getFilterDefaultValue(filter))}
            >
              {filter.label}: {formatFilterChipValue(values[filter.id])}
              <X className="ml-2 h-3 w-3" />
            </Button>
          ))}
        </div>
      )}
    </div>
  );
}

function FilterControl({
  reportId,
  filter,
  value,
  allValues,
  onChange,
}: {
  reportId?: string;
  filter: ReportFilterDefinition;
  value: unknown;
  allValues: Record<string, unknown>;
  onChange: (value: unknown) => void;
}) {
  const usesDynamicOptions = filter.options?.source === 'object_model';
  const optionRequest = useMemo(
    () => ({
      filters: allValues,
      limit: 200,
      timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
    }),
    [allValues]
  );
  const { data: dynamicOptions, isFetching: isLoadingOptions } =
    useReportFilterOptions(
      reportId,
      filter.id,
      optionRequest,
      Boolean(reportId && usesDynamicOptions)
    );
  const options = dynamicOptions?.options ?? filter.options?.values ?? [];

  if (filter.type === 'time_range') {
    return (
      <div className="min-w-44 space-y-1">
        <Label className="text-xs font-medium">{filter.label}</Label>
        <Select
          value={String(value ?? 'last_30_days')}
          onValueChange={onChange}
        >
          <SelectTrigger className="h-9">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {TIME_RANGE_PRESETS.map((option) => (
              <SelectItem key={option.value} value={option.value}>
                {option.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
    );
  }

  if (filter.type === 'select' || filter.type === 'radio') {
    const selectedKey = optionKey(value);
    return (
      <div className="min-w-44 space-y-1">
        <Label className="text-xs font-medium">{filter.label}</Label>
        <Select
          value={selectedKey}
          onValueChange={(nextKey) => {
            const option = options.find(
              (option) => optionKey(option.value) === nextKey
            );
            onChange(option?.value ?? nextKey);
          }}
        >
          <SelectTrigger className="h-9">
            <SelectValue
              placeholder={isLoadingOptions ? 'Loading...' : 'Any'}
            />
          </SelectTrigger>
          <SelectContent>
            {options.map((option) => (
              <SelectItem
                key={optionKey(option.value)}
                value={optionKey(option.value)}
              >
                {formatOptionLabel(option.label, option.count)}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
    );
  }

  if (filter.type === 'multi_select') {
    const selected = Array.isArray(value) ? value.map(optionKey) : [];

    return (
      <div className="min-w-56 space-y-1">
        <Label className="text-xs font-medium">{filter.label}</Label>
        <div className="flex min-h-9 flex-wrap items-center gap-2 rounded-md border border-input bg-background px-3 py-2">
          {options.map((option) => {
            const optionValueKey = optionKey(option.value);
            const checked = selected.includes(optionValueKey);
            return (
              <label
                key={optionValueKey}
                className="flex items-center gap-2 text-sm text-muted-foreground"
              >
                <Checkbox
                  checked={checked}
                  onCheckedChange={(next) => {
                    const currentValues = Array.isArray(value) ? value : [];
                    const nextSelected = next
                      ? [...currentValues, option.value]
                      : currentValues.filter(
                          (item) => optionKey(item) !== optionValueKey
                        );
                    onChange(nextSelected);
                  }}
                />
                {formatOptionLabel(option.label, option.count)}
              </label>
            );
          })}
        </div>
      </div>
    );
  }

  if (filter.type === 'checkbox') {
    return (
      <label className="flex h-9 items-center gap-2 rounded-md border border-input bg-background px-3 text-sm">
        <Checkbox
          checked={Boolean(value)}
          onCheckedChange={(next) => onChange(Boolean(next))}
        />
        {filter.label}
      </label>
    );
  }

  return (
    <div className="min-w-56 space-y-1">
      <Label className="text-xs font-medium">{filter.label}</Label>
      <Input
        className="h-9"
        value={String(value ?? '')}
        onChange={(event) => onChange(event.target.value)}
      />
    </div>
  );
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

function isFilterDefault(
  filter: ReportFilterDefinition,
  value: unknown
): boolean {
  return (
    JSON.stringify(value ?? getFilterDefaultValue(filter)) ===
    JSON.stringify(getFilterDefaultValue(filter))
  );
}

function formatFilterChipValue(value: unknown): string {
  if (Array.isArray(value)) return value.map(String).join(', ');
  if (typeof value === 'boolean') return value ? 'Yes' : 'No';
  if (value === null || value === undefined || value === '') return 'Any';
  return String(value);
}
