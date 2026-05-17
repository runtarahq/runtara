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
import {
  ReportDefinition,
  ReportFilterDefinition,
  ReportFilterType,
} from '../../types';

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

interface FiltersEditorV2Props {
  definition: ReportDefinition;
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
  onChange,
}: FiltersEditorV2Props) {
  const filters = definition.filters ?? [];

  const updateFilters = (next: ReportFilterDefinition[]) =>
    onChange({ ...definition, filters: next });

  const updateFilter = (
    id: string,
    updater: (filter: ReportFilterDefinition) => ReportFilterDefinition
  ) =>
    updateFilters(filters.map((f) => (f.id === id ? updater(f) : f)));

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
                <div className="grid grid-cols-2 gap-3">
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

                <label className="flex items-center gap-2 text-xs">
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
