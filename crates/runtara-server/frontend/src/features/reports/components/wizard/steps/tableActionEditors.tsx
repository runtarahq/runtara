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
import { useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import { getWorkflows } from '@/features/workflows/queries';
import { WorkflowDto } from '@/generated/RuntaraRuntimeApi';
import {
  ReportInteractionAction,
  ReportTableActionConfig,
  ReportTableInteractionButtonConfig,
  ReportWorkflowActionConfig,
  ReportWorkflowActionContextMode,
} from '../../../types';
import { slugify } from '../../../utils';

const WORKFLOW_CONTEXT_MODES: Array<{
  value: ReportWorkflowActionContextMode;
  label: string;
}> = [
  { value: 'row', label: 'Row (per-row context)' },
  { value: 'field', label: 'Field (single field value)' },
  { value: 'value', label: 'Value (cell value)' },
  { value: 'selection', label: 'Selection (bulk)' },
];

const INTERACTION_BUTTON_ICONS: Array<{
  value: NonNullable<ReportTableInteractionButtonConfig['icon']>;
  label: string;
}> = [
  { value: 'arrow_right', label: 'Arrow right' },
  { value: 'eye', label: 'Eye' },
  { value: 'file_text', label: 'File text' },
  { value: 'activity', label: 'Activity' },
];

const INTERACTION_ACTION_TYPES: Array<{
  value: ReportInteractionAction['type'];
  label: string;
}> = [
  { value: 'set_filter', label: 'Set filter' },
  { value: 'clear_filter', label: 'Clear filter' },
  { value: 'clear_filters', label: 'Clear filters' },
  { value: 'navigate_view', label: 'Navigate to view' },
];

type WorkflowOption = { id: string; name: string };

function useWorkflowOptions() {
  // Cast through `any` because useCustomQuery types `select` as
  // (data: TData) => TData, but we want a narrowed shape — same trick used
  // elsewhere in the codebase (CreateTrigger etc.).
  return useCustomQuery({
    queryKey: queryKeys.workflows.all,
    queryFn: getWorkflows,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    select: (response: any): WorkflowOption[] => {
      const content: WorkflowDto[] = response?.data?.content ?? [];
      return content.map((workflow) => ({
        id: workflow.id,
        name: workflow.name || workflow.id,
      }));
    },
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
  }) as unknown as { data?: WorkflowOption[]; isFetching: boolean } & Record<
    string,
    any
  >;
}

export function createDefaultWorkflowAction(
  mode: ReportWorkflowActionContextMode = 'row'
): ReportWorkflowActionConfig {
  return {
    workflowId: '',
    label: 'Run workflow',
    reloadBlock: true,
    context: { mode },
  };
}

export function createDefaultInteractionButton(): ReportTableInteractionButtonConfig {
  return {
    id: `btn_${Math.random().toString(36).slice(2, 7)}`,
    label: 'Open',
    icon: 'arrow_right',
    actions: [{ type: 'set_filter', valueFrom: 'datum.id' }],
  };
}

export function createDefaultTableAction(): ReportTableActionConfig {
  return {
    id: `bulk_${Math.random().toString(36).slice(2, 7)}`,
    label: 'Run on selection',
    workflowAction: createDefaultWorkflowAction('selection'),
  };
}

/** Compact workflow-action editor used inline in the wizard. */
export function WorkflowActionEditor({
  action,
  onChange,
  contextOptions = WORKFLOW_CONTEXT_MODES,
  fields = [],
}: {
  action: ReportWorkflowActionConfig;
  onChange: (action: ReportWorkflowActionConfig) => void;
  /** When set, restricts the context modes a user can pick. */
  contextOptions?: typeof WORKFLOW_CONTEXT_MODES;
  /** Row fields offered for visibleWhen/disabledWhen field pickers. */
  fields?: string[];
}) {
  const workflows = useWorkflowOptions();
  const context = action.context ?? {};
  const update = (patch: Partial<ReportWorkflowActionConfig>) =>
    onChange({ ...action, ...patch });

  return (
    <div className="grid gap-2 rounded-md border bg-muted/10 p-3">
      <div className="grid gap-2 sm:grid-cols-2">
        <div className="grid gap-1">
          <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Workflow
          </Label>
          <Select
            value={action.workflowId || ''}
            onValueChange={(workflowId) => update({ workflowId })}
            disabled={workflows.isFetching}
          >
            <SelectTrigger className="h-8">
              <SelectValue
                placeholder={
                  workflows.isFetching ? 'Loading…' : 'Select workflow'
                }
              />
            </SelectTrigger>
            <SelectContent>
              {(workflows.data ?? []).map((workflow) => (
                <SelectItem key={workflow.id} value={workflow.id}>
                  {workflow.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <div className="grid gap-1">
          <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Context mode
          </Label>
          <Select
            value={context.mode ?? 'row'}
            onValueChange={(mode) =>
              update({
                context: {
                  ...context,
                  mode: mode as ReportWorkflowActionContextMode,
                },
              })
            }
          >
            <SelectTrigger className="h-8">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {contextOptions.map((option) => (
                <SelectItem key={option.value} value={option.value}>
                  {option.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
      </div>
      <div className="grid gap-2 sm:grid-cols-3">
        <div className="grid gap-1">
          <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Button label
          </Label>
          <Input
            value={action.label ?? ''}
            placeholder="Run workflow"
            className="h-8"
            onChange={(event) => update({ label: event.target.value })}
          />
        </div>
        <div className="grid gap-1">
          <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Running label
          </Label>
          <Input
            value={action.runningLabel ?? ''}
            placeholder="Running…"
            className="h-8"
            onChange={(event) =>
              update({ runningLabel: event.target.value || undefined })
            }
          />
        </div>
        <div className="grid gap-1">
          <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
            Success message
          </Label>
          <Input
            value={action.successMessage ?? ''}
            placeholder="Started"
            className="h-8"
            onChange={(event) =>
              update({ successMessage: event.target.value || undefined })
            }
          />
        </div>
      </div>
      <label className="flex items-center gap-2 text-sm">
        <Checkbox
          checked={Boolean(action.reloadBlock)}
          onCheckedChange={(checked) =>
            update({ reloadBlock: Boolean(checked) })
          }
        />
        Reload block after the workflow finishes
      </label>
      <RowConditionRow
        label="Visible when"
        value={action.visibleWhen}
        fields={fields}
        onChange={(visibleWhen) => update({ visibleWhen })}
      />
      <RowConditionRow
        label="Disabled when"
        value={action.disabledWhen}
        fields={fields}
        onChange={(disabledWhen) => update({ disabledWhen })}
      />
    </div>
  );
}

const ROW_CONDITION_FIELD_NONE = '__none__';

/** Minimal row-condition row — supports the common `EQ field value` form
 *  inline; advanced conditions are still preserved by the round-trip but
 *  surface here as a read-only summary. */
function RowConditionRow({
  label,
  value,
  fields,
  onChange,
}: {
  label: string;
  value: NonNullable<ReportWorkflowActionConfig['visibleWhen']> | undefined;
  fields: string[];
  onChange: (
    value: NonNullable<ReportWorkflowActionConfig['visibleWhen']> | undefined
  ) => void;
}) {
  const isSimple =
    !!value &&
    typeof value.op === 'string' &&
    value.op.toUpperCase() === 'EQ' &&
    Array.isArray(value.arguments) &&
    value.arguments.length === 2 &&
    typeof value.arguments[0] === 'string' &&
    !isObjectArg(value.arguments[1]);
  const simpleField = isSimple ? String(value!.arguments![0]) : '';
  const simpleValue =
    isSimple && value!.arguments![1] !== undefined
      ? String(value!.arguments![1])
      : '';
  // If the stored field isn't in the offered list, expose it as an option so
  // we don't silently drop it on edit.
  const fieldOptions =
    simpleField && !fields.includes(simpleField)
      ? [simpleField, ...fields]
      : fields;

  if (value && !isSimple) {
    return (
      <div className="grid gap-1 rounded-md border bg-background p-2 text-xs text-muted-foreground">
        <span className="font-semibold uppercase tracking-wider">{label}</span>
        <code className="truncate">{summarizeCondition(value)}</code>
        <div>
          <Button
            type="button"
            size="sm"
            variant="ghost"
            className="h-6 px-2"
            onClick={() => onChange(undefined)}
          >
            Clear
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className="grid gap-1">
      <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
        {label}
      </Label>
      <div className="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] gap-1">
        {fieldOptions.length > 0 ? (
          <Select
            value={simpleField || ROW_CONDITION_FIELD_NONE}
            onValueChange={(field) => {
              if (field === ROW_CONDITION_FIELD_NONE) {
                onChange(undefined);
                return;
              }
              onChange({
                op: 'EQ',
                arguments: [field, simpleValue],
              });
            }}
          >
            <SelectTrigger className="h-8">
              <SelectValue placeholder="Field" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={ROW_CONDITION_FIELD_NONE}>— none —</SelectItem>
              {fieldOptions.map((option) => (
                <SelectItem key={option} value={option}>
                  {option}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        ) : (
          <Input
            value={simpleField}
            placeholder="field"
            className="h-8"
            onChange={(event) => {
              const field = event.target.value;
              if (!field) {
                onChange(undefined);
                return;
              }
              onChange({
                op: 'EQ',
                arguments: [field, simpleValue],
              });
            }}
          />
        )}
        <Input
          value={simpleValue}
          placeholder="value"
          className="h-8"
          disabled={!simpleField}
          onChange={(event) => {
            if (!simpleField) return;
            onChange({
              op: 'EQ',
              arguments: [simpleField, event.target.value],
            });
          }}
        />
        <Button
          type="button"
          size="icon"
          variant="ghost"
          className="h-8 w-8"
          disabled={!value}
          onClick={() => onChange(undefined)}
          aria-label={`Clear ${label.toLowerCase()}`}
        >
          <Trash2 className="h-3.5 w-3.5" />
        </Button>
      </div>
    </div>
  );
}

function isObjectArg(value: unknown): boolean {
  return typeof value === 'object' && value !== null;
}

function summarizeCondition(
  condition: NonNullable<ReportWorkflowActionConfig['visibleWhen']>
): string {
  try {
    return JSON.stringify(condition);
  } catch {
    return '(complex condition preserved on save)';
  }
}

export function InteractionButtonsEditor({
  buttons,
  fields,
  onChange,
}: {
  buttons: ReportTableInteractionButtonConfig[];
  /** Field options offered for the `set_filter` action's `value from` selector. */
  fields: string[];
  onChange: (buttons: ReportTableInteractionButtonConfig[]) => void;
}) {
  const updateButton = (
    index: number,
    patch: Partial<ReportTableInteractionButtonConfig>
  ) => {
    onChange(
      buttons.map((button, currentIndex) =>
        currentIndex === index ? { ...button, ...patch } : button
      )
    );
  };

  return (
    <div className="grid gap-2 rounded-md border bg-muted/10 p-3">
      <div className="flex items-center justify-between">
        <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
          Buttons
        </Label>
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-7"
          onClick={() =>
            onChange([...buttons, createDefaultInteractionButton()])
          }
        >
          <Plus className="mr-1 h-3 w-3" />
          Add button
        </Button>
      </div>
      {buttons.length === 0 ? (
        <p className="text-xs text-muted-foreground">
          No buttons yet — add one to render an interaction.
        </p>
      ) : (
        <div className="grid gap-2">
          {buttons.map((button, index) => (
            <div
              key={`${button.id}-${index}`}
              className="rounded-md border bg-background p-2"
            >
              <div className="grid gap-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_140px_auto]">
                <div className="grid gap-1">
                  <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                    ID
                  </Label>
                  <Input
                    value={button.id}
                    className="h-8"
                    onChange={(event) =>
                      updateButton(index, {
                        id: slugify(event.target.value).replace(/-/g, '_'),
                      })
                    }
                  />
                </div>
                <div className="grid gap-1">
                  <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                    Label
                  </Label>
                  <Input
                    value={button.label ?? ''}
                    className="h-8"
                    placeholder="Open"
                    onChange={(event) =>
                      updateButton(index, { label: event.target.value })
                    }
                  />
                </div>
                <div className="grid gap-1">
                  <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                    Icon
                  </Label>
                  <Select
                    value={button.icon ?? 'arrow_right'}
                    onValueChange={(icon) =>
                      updateButton(index, {
                        icon: icon as NonNullable<typeof button.icon>,
                      })
                    }
                  >
                    <SelectTrigger className="h-8">
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      {INTERACTION_BUTTON_ICONS.map((option) => (
                        <SelectItem key={option.value} value={option.value}>
                          {option.label}
                        </SelectItem>
                      ))}
                    </SelectContent>
                  </Select>
                </div>
                <Button
                  type="button"
                  size="icon"
                  variant="ghost"
                  className="mt-5 h-8 w-8"
                  onClick={() =>
                    onChange(
                      buttons.filter(
                        (_, currentIndex) => currentIndex !== index
                      )
                    )
                  }
                  aria-label={`Remove ${button.label || 'button'}`}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              </div>
              <InteractionActionsList
                actions={button.actions}
                fields={fields}
                onChange={(actions) => updateButton(index, { actions })}
              />
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

export function InteractionActionsList({
  actions,
  fields,
  onChange,
}: {
  actions: ReportInteractionAction[];
  fields: string[];
  onChange: (actions: ReportInteractionAction[]) => void;
}) {
  const updateAction = (
    index: number,
    patch: Partial<ReportInteractionAction>
  ) => {
    onChange(
      actions.map((action, currentIndex) =>
        currentIndex === index ? { ...action, ...patch } : action
      )
    );
  };

  return (
    <div className="mt-2 grid gap-2">
      <div className="flex items-center justify-between">
        <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
          Actions
        </Label>
        <Button
          type="button"
          size="sm"
          variant="outline"
          className="h-7"
          onClick={() =>
            onChange([
              ...actions,
              {
                type: 'set_filter',
                valueFrom: 'datum.id',
              } as ReportInteractionAction,
            ])
          }
        >
          <Plus className="mr-1 h-3 w-3" />
          Add action
        </Button>
      </div>
      {actions.length === 0 ? (
        <p className="text-xs text-muted-foreground">No actions configured.</p>
      ) : (
        actions.map((action, index) => (
          <div
            key={`action-${index}`}
            className="grid gap-2 rounded-md border bg-muted/10 p-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto]"
          >
            <div className="grid gap-1">
              <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                Action
              </Label>
              <Select
                value={action.type}
                onValueChange={(type) =>
                  updateAction(index, {
                    ...action,
                    type,
                    ...(type === 'set_filter'
                      ? {
                          valueFrom:
                            action.valueFrom ?? `datum.${fields[0] ?? 'id'}`,
                        }
                      : {}),
                  })
                }
              >
                <SelectTrigger className="h-8">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {INTERACTION_ACTION_TYPES.map((option) => (
                    <SelectItem key={option.value} value={option.value}>
                      {option.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            {(action.type === 'set_filter' ||
              action.type === 'clear_filter') && (
              <div className="grid gap-1">
                <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                  Filter ID
                </Label>
                <Input
                  value={action.filterId ?? ''}
                  placeholder="filter_id"
                  className="h-8"
                  onChange={(event) =>
                    updateAction(index, {
                      filterId: event.target.value || undefined,
                    })
                  }
                />
              </div>
            )}
            {action.type === 'clear_filters' && (
              <div className="grid gap-1">
                <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                  Filter IDs (comma-separated)
                </Label>
                <Input
                  value={(action.filterIds ?? []).join(', ')}
                  placeholder="filter_a, filter_b"
                  className="h-8"
                  onChange={(event) =>
                    updateAction(index, {
                      filterIds: event.target.value
                        .split(',')
                        .map((part) => part.trim())
                        .filter(Boolean),
                    })
                  }
                />
              </div>
            )}
            {action.type === 'navigate_view' && (
              <div className="grid gap-1">
                <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                  View ID
                </Label>
                <Input
                  value={action.viewId ?? ''}
                  placeholder="view_id"
                  className="h-8"
                  onChange={(event) =>
                    updateAction(index, {
                      viewId: event.target.value || undefined,
                    })
                  }
                />
              </div>
            )}
            {action.type === 'set_filter' && (
              <div className="grid gap-1 sm:col-span-2">
                <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                  Value from
                </Label>
                <Input
                  value={action.valueFrom ?? ''}
                  placeholder="datum.id"
                  className="h-8"
                  onChange={(event) =>
                    updateAction(index, {
                      valueFrom: event.target.value || undefined,
                    })
                  }
                />
              </div>
            )}
            <Button
              type="button"
              size="icon"
              variant="ghost"
              className="mt-5 h-8 w-8"
              onClick={() =>
                onChange(
                  actions.filter((_, currentIndex) => currentIndex !== index)
                )
              }
              aria-label="Remove action"
            >
              <Trash2 className="h-3.5 w-3.5" />
            </Button>
          </div>
        ))
      )}
    </div>
  );
}

export function TableBulkActionsEditor({
  actions,
  fields = [],
  onChange,
}: {
  actions: ReportTableActionConfig[];
  /** Row fields offered for visibleWhen/disabledWhen field pickers. */
  fields?: string[];
  onChange: (actions: ReportTableActionConfig[]) => void;
}) {
  const updateAction = (
    index: number,
    patch: Partial<ReportTableActionConfig>
  ) => {
    onChange(
      actions.map((action, currentIndex) =>
        currentIndex === index ? { ...action, ...patch } : action
      )
    );
  };

  return (
    <div className="grid gap-2">
      <div className="flex items-center justify-between">
        <Label className="text-[11px] font-semibold uppercase tracking-wider text-muted-foreground">
          Bulk actions
        </Label>
        <Button
          type="button"
          size="sm"
          variant="outline"
          className="h-7"
          onClick={() => onChange([...actions, createDefaultTableAction()])}
        >
          <Plus className="mr-1 h-3 w-3" />
          Add bulk action
        </Button>
      </div>
      {actions.length === 0 ? (
        <p className="text-xs text-muted-foreground">
          No bulk actions yet. Bulk actions run a workflow on the rows the
          viewer has selected.
        </p>
      ) : (
        actions.map((action, index) => (
          <div
            key={`${action.id}-${index}`}
            className="rounded-md border bg-background p-3"
          >
            <div className="grid gap-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto]">
              <div className="grid gap-1">
                <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                  ID
                </Label>
                <Input
                  value={action.id}
                  className="h-8"
                  onChange={(event) =>
                    updateAction(index, {
                      id: slugify(event.target.value).replace(/-/g, '_'),
                    })
                  }
                />
              </div>
              <div className="grid gap-1">
                <Label className="text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                  Label
                </Label>
                <Input
                  value={action.label ?? ''}
                  className="h-8"
                  placeholder="Run on selection"
                  onChange={(event) =>
                    updateAction(index, { label: event.target.value })
                  }
                />
              </div>
              <Button
                type="button"
                size="icon"
                variant="ghost"
                className="mt-5 h-8 w-8"
                onClick={() =>
                  onChange(
                    actions.filter((_, currentIndex) => currentIndex !== index)
                  )
                }
                aria-label={`Remove ${action.label || 'bulk action'}`}
              >
                <Trash2 className="h-3.5 w-3.5" />
              </Button>
            </div>
            <div className="mt-2">
              <WorkflowActionEditor
                action={action.workflowAction}
                contextOptions={WORKFLOW_CONTEXT_MODES.filter(
                  (option) => option.value === 'selection'
                )}
                fields={fields}
                onChange={(workflowAction) =>
                  updateAction(index, { workflowAction })
                }
              />
            </div>
          </div>
        ))
      )}
    </div>
  );
}
