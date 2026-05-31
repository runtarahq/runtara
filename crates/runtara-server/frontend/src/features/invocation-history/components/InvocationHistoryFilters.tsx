import { ExecutionStatus } from '@/generated/RuntaraRuntimeApi';
import { WorkflowDto } from '@/generated/RuntaraRuntimeApi';
import { ExecutionHistoryFilters } from '../types';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { getWorkflows } from '@/features/workflows/queries';

interface Props {
  filters: ExecutionHistoryFilters;
  onFiltersChange: (filters: ExecutionHistoryFilters) => void;
}

const ALL_VALUE = '__all__';

/** Count of active (non-sort) filters — drives the toolbar filter badge. */
export function countActiveInvocationFilters(
  filters: ExecutionHistoryFilters
): number {
  return [
    filters.workflowId,
    filters.status,
    filters.createdFrom,
    filters.createdTo,
    filters.completedFrom,
    filters.completedTo,
  ].filter((value) => value !== undefined && value !== null && value !== '')
    .length;
}

// Convert datetime-local input value to ISO 8601 string for API
const toISOString = (datetimeLocalValue: string): string | undefined => {
  if (!datetimeLocalValue) return undefined;
  const date = new Date(datetimeLocalValue);
  return date.toISOString();
};

// Convert ISO 8601 string from API to datetime-local input value
const toDatetimeLocal = (isoString: string | undefined): string => {
  if (!isoString) return '';
  const date = new Date(isoString);
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

/**
 * Vertical filter body, designed to live inside a <FilterPopover />.
 */
export function InvocationHistoryFilters({ filters, onFiltersChange }: Props) {
  const { data: workflowsResponse } = useCustomQuery({
    queryKey: queryKeys.workflows.all,
    queryFn: getWorkflows,
  });

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

  return (
    <div className="space-y-3">
      <div className="space-y-1.5">
        <Label className="text-xs text-muted-foreground">Workflow</Label>
        <Select
          value={filters.workflowId || ALL_VALUE}
          onValueChange={handleWorkflowChange}
        >
          <SelectTrigger className="h-8 w-full text-sm">
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

      <div className="space-y-1.5">
        <Label className="text-xs text-muted-foreground">Status</Label>
        <Select
          value={filters.status || ALL_VALUE}
          onValueChange={handleStatusChange}
        >
          <SelectTrigger className="h-8 w-full text-sm">
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

      <div className="grid grid-cols-2 gap-2">
        <div className="space-y-1.5">
          <Label
            htmlFor="created-from"
            className="text-xs text-muted-foreground"
          >
            Created from
          </Label>
          <Input
            id="created-from"
            type="datetime-local"
            className="h-8 bg-background text-sm"
            value={toDatetimeLocal(filters.createdFrom)}
            onChange={(e) => handleDateChange('createdFrom', e.target.value)}
          />
        </div>
        <div className="space-y-1.5">
          <Label htmlFor="created-to" className="text-xs text-muted-foreground">
            Created to
          </Label>
          <Input
            id="created-to"
            type="datetime-local"
            className="h-8 bg-background text-sm"
            value={toDatetimeLocal(filters.createdTo)}
            onChange={(e) => handleDateChange('createdTo', e.target.value)}
          />
        </div>
        <div className="space-y-1.5">
          <Label
            htmlFor="completed-from"
            className="text-xs text-muted-foreground"
          >
            Completed from
          </Label>
          <Input
            id="completed-from"
            type="datetime-local"
            className="h-8 bg-background text-sm"
            value={toDatetimeLocal(filters.completedFrom)}
            onChange={(e) => handleDateChange('completedFrom', e.target.value)}
          />
        </div>
        <div className="space-y-1.5">
          <Label
            htmlFor="completed-to"
            className="text-xs text-muted-foreground"
          >
            Completed to
          </Label>
          <Input
            id="completed-to"
            type="datetime-local"
            className="h-8 bg-background text-sm"
            value={toDatetimeLocal(filters.completedTo)}
            onChange={(e) => handleDateChange('completedTo', e.target.value)}
          />
        </div>
      </div>
    </div>
  );
}
