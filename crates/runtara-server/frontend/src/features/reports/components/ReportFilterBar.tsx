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
import { TIME_RANGE_PRESETS } from '../utils';

type ReportFilterBarProps = {
  definition: ReportDefinition;
  values: Record<string, unknown>;
  onChange: (filterId: string, value: unknown) => void;
};

export function ReportFilterBar({
  definition,
  values,
  onChange,
}: ReportFilterBarProps) {
  if (definition.filters.length === 0) {
    return null;
  }

  return (
    <div className="flex flex-wrap items-end gap-3">
      {definition.filters.map((filter) => (
        <FilterControl
          key={filter.id}
          filter={filter}
          value={values[filter.id]}
          onChange={(value) => onChange(filter.id, value)}
        />
      ))}
    </div>
  );
}

function FilterControl({
  filter,
  value,
  onChange,
}: {
  filter: ReportFilterDefinition;
  value: unknown;
  onChange: (value: unknown) => void;
}) {
  const options = filter.options?.values ?? [];

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
    return (
      <div className="min-w-44 space-y-1">
        <Label className="text-xs font-medium">{filter.label}</Label>
        <Select value={String(value ?? '')} onValueChange={onChange}>
          <SelectTrigger className="h-9">
            <SelectValue placeholder="Any" />
          </SelectTrigger>
          <SelectContent>
            {options.map((option) => (
              <SelectItem
                key={String(option.value)}
                value={String(option.value)}
              >
                {option.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
    );
  }

  if (filter.type === 'multi_select') {
    const selected = Array.isArray(value) ? value.map(String) : [];

    return (
      <div className="min-w-56 space-y-1">
        <Label className="text-xs font-medium">{filter.label}</Label>
        <div className="flex min-h-9 flex-wrap items-center gap-2 rounded-md border border-input bg-background px-3 py-2">
          {options.map((option) => {
            const optionValue = String(option.value);
            const checked = selected.includes(optionValue);
            return (
              <label
                key={optionValue}
                className="flex items-center gap-2 text-sm text-muted-foreground"
              >
                <Checkbox
                  checked={checked}
                  onCheckedChange={(next) => {
                    const nextSelected = next
                      ? [...selected, optionValue]
                      : selected.filter((item) => item !== optionValue);
                    onChange(nextSelected);
                  }}
                />
                {option.label}
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
