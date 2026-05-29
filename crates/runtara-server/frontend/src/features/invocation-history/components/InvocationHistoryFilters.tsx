import { useState } from 'react';
import { Filter, X, ChevronDown } from 'lucide-react';
import { ExecutionStatus } from '@/generated/RuntaraRuntimeApi';
import { WorkflowDto } from '@/generated/RuntaraRuntimeApi';
import { ExecutionHistoryFilters } from '../types';
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
  Collapsible,
  CollapsibleContent,
} from '@/shared/components/ui/collapsible';
import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { getWorkflows } from '@/features/workflows/queries';

interface Props {
  filters: ExecutionHistoryFilters;
  onFiltersChange: (filters: ExecutionHistoryFilters) => void;
}

const ALL_VALUE = '__all__';

// Convert datetime-local input value to ISO 8601 string for API
const toISOString = (datetimeLocalValue: string): string | undefined => {
  if (!datetimeLocalValue) return undefined;
  // datetime-local returns "2024-01-15T10:30", we need to convert to ISO with timezone
  const date = new Date(datetimeLocalValue);
  return date.toISOString();
};

// Convert ISO 8601 string from API to datetime-local input value
const toDatetimeLocal = (isoString: string | undefined): string => {
  if (!isoString) return '';
  // ISO string is "2024-01-15T10:30:00.000Z", datetime-local needs "2024-01-15T10:30"
  const date = new Date(isoString);
  // Format as YYYY-MM-DDTHH:mm in local timezone
  const year = date.getFullYear();
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  const hours = String(date.getHours()).padStart(2, '0');
  const minutes = String(date.getMinutes()).padStart(2, '0');
  return `${year}-${month}-${day}T${hours}:${minutes}`;
};

const STATUS_OPTIONS: {
  value: ExecutionStatus | typeof ALL_VALUE;
  label: string;
}[] = [
  { value: ALL_VALUE, label: 'All statuses' },
  { value: 'queued', label: 'Queued' },
  { value: 'running', label: 'Running' },
  { value: 'completed', label: 'Completed' },
  { value: 'failed', label: 'Failed' },
  { value: 'cancelled', label: 'Cancelled' },
];

export function InvocationHistoryFilters({ filters, onFiltersChange }: Props) {
  const [isOpen, setIsOpen] = useState(false);

  // Fetch workflows for the dropdown
  const { data: workflowsResponse } = useCustomQuery({
    queryKey: queryKeys.workflows.all,
    queryFn: getWorkflows,
  });

  // Extract from paginated response: { data: { content: WorkflowDto[], ... } }
  const workflows = ((workflowsResponse as any)?.data?.content ||
    []) as WorkflowDto[];

  const handleWorkflowChange = (value: string) => {
    onFiltersChange({
      ...filters,
      workflowId: value === ALL_VALUE ? undefined : value,
    });
  };

  const handleStatusChange = (value: string) => {
    onFiltersChange({
      ...filters,
      status: value === ALL_VALUE ? undefined : value,
    });
  };

  const handleDateChange = (
    field: 'createdFrom' | 'createdTo' | 'completedFrom' | 'completedTo',
    value: string
  ) => {
    onFiltersChange({
      ...filters,
      [field]: toISOString(value),
    });
  };

  const handleClearFilters = () => {
    onFiltersChange({
      ...filters,
      status: undefined,
      workflowId: undefined,
      createdFrom: undefined,
      createdTo: undefined,
      completedFrom: undefined,
      completedTo: undefined,
    });
  };

  const hasDateFilters =
    filters.createdFrom ||
    filters.createdTo ||
    filters.completedFrom ||
    filters.completedTo;

  const hasActiveFilters =
    filters.status || filters.workflowId || hasDateFilters;

  return (
    <div className="mb-6 space-y-4">
      {/* Primary filters row */}
      <div className="flex flex-wrap items-center gap-3">
        {/* Workflow filter */}
        <div className="flex items-center gap-2">
          <span className="text-sm text-muted-foreground">
            Workflow:
          </span>
          <Select
            value={filters.workflowId || ALL_VALUE}
            onValueChange={handleWorkflowChange}
          >
            <SelectTrigger className="h-8 min-w-[160px] text-sm">
              <SelectValue placeholder="All workflows" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={ALL_VALUE}>All workflows</SelectItem>
              {workflows.map((workflow) => (
                <SelectItem
                  key={workflow.id}
                  value={workflow.id || `workflow-${workflow.name}`}
                >
                  {workflow.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        {/* Status filter */}
        <div className="flex items-center gap-2">
          <span className="text-sm text-muted-foreground">
            Status:
          </span>
          <Select
            value={filters.status || ALL_VALUE}
            onValueChange={handleStatusChange}
          >
            <SelectTrigger className="h-8 min-w-[140px] text-sm">
              <SelectValue placeholder="All statuses" />
            </SelectTrigger>
            <SelectContent>
              {STATUS_OPTIONS.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        {/* Date filters toggle */}
        <button
          onClick={() => setIsOpen(!isOpen)}
          className="inline-flex items-center gap-2 h-8 px-3 text-sm text-foreground border rounded-md hover:bg-muted transition-colors"
        >
          <Filter className="w-4 h-4" />
          <span>Date filters</span>
          {hasDateFilters && (
            <span className="rounded-full bg-primary px-2 py-0.5 text-xs text-primary-foreground">
              Active
            </span>
          )}
          <ChevronDown
            className={`w-4 h-4 text-muted-foreground transition-transform ${isOpen ? 'rotate-180' : ''}`}
          />
        </button>

        {/* Spacer */}
        <div className="flex-1" />

        {/* Clear filters button */}
        {hasActiveFilters && (
          <Button
            variant="ghost"
            size="sm"
            onClick={handleClearFilters}
            className="gap-1 text-muted-foreground hover:text-foreground"
          >
            <X className="h-4 w-4" />
            Clear filters
          </Button>
        )}
      </div>

      {/* Date filters - collapsible */}
      <Collapsible open={isOpen} onOpenChange={setIsOpen}>
        <CollapsibleContent>
          <div className="grid grid-cols-1 gap-4 rounded-lg border bg-muted/30 p-4 sm:grid-cols-2 lg:grid-cols-4">
            <div className="space-y-2">
              <Label
                htmlFor="created-from"
                className="text-sm text-muted-foreground"
              >
                Created from
              </Label>
              <Input
                id="created-from"
                type="datetime-local"
                value={toDatetimeLocal(filters.createdFrom)}
                onChange={(e) =>
                  handleDateChange('createdFrom', e.target.value)
                }
                className="bg-background"
              />
            </div>

            <div className="space-y-2">
              <Label
                htmlFor="created-to"
                className="text-sm text-muted-foreground"
              >
                Created to
              </Label>
              <Input
                id="created-to"
                type="datetime-local"
                value={toDatetimeLocal(filters.createdTo)}
                onChange={(e) => handleDateChange('createdTo', e.target.value)}
                className="bg-background"
              />
            </div>

            <div className="space-y-2">
              <Label
                htmlFor="completed-from"
                className="text-sm text-muted-foreground"
              >
                Completed from
              </Label>
              <Input
                id="completed-from"
                type="datetime-local"
                value={toDatetimeLocal(filters.completedFrom)}
                onChange={(e) =>
                  handleDateChange('completedFrom', e.target.value)
                }
                className="bg-background"
              />
            </div>

            <div className="space-y-2">
              <Label
                htmlFor="completed-to"
                className="text-sm text-muted-foreground"
              >
                Completed to
              </Label>
              <Input
                id="completed-to"
                type="datetime-local"
                value={toDatetimeLocal(filters.completedTo)}
                onChange={(e) =>
                  handleDateChange('completedTo', e.target.value)
                }
                className="bg-background"
              />
            </div>
          </div>
        </CollapsibleContent>
      </Collapsible>
    </div>
  );
}
