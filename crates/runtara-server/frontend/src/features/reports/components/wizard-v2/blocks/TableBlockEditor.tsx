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
import { Plus, Trash2 } from 'lucide-react';
import { Schema } from '@/generated/RuntaraRuntimeApi';
import {
  ReportBlockDefinition,
  ReportTableColumn,
  ReportTableColumnType,
} from '../../../types';
import { humanizeFieldName } from '../../../utils';
import {
  InteractionActionsList,
  InteractionButtonsEditor,
  TableBulkActionsEditor,
  WorkflowActionEditor,
  createDefaultInteractionButton,
  createDefaultTableAction,
  createDefaultWorkflowAction,
} from './tableActionEditors';

const PLAIN = '__plain__';
const FORMATS: Array<{ value: string; label: string }> = [
  { value: PLAIN, label: 'Plain' },
  { value: 'number', label: 'Number' },
  { value: 'decimal', label: 'Decimal' },
  { value: 'currency', label: 'Currency' },
  { value: 'percent', label: 'Percent' },
  { value: 'date', label: 'Date' },
  { value: 'datetime', label: 'Date + time' },
  { value: 'pill', label: 'Pill' },
];

const COLUMN_TYPES: Array<{ value: ReportTableColumnType; label: string }> = [
  { value: 'value', label: 'Value' },
  { value: 'workflow_button', label: 'Workflow button' },
  { value: 'interaction_buttons', label: 'Interaction buttons' },
];

interface TableBlockEditorProps {
  block: ReportBlockDefinition;
  schemas: Schema[];
  onChange: (block: ReportBlockDefinition) => void;
}

export function TableBlockEditor({
  block,
  schemas,
  onChange,
}: TableBlockEditorProps) {
  const table = block.table ?? { columns: [] };
  const columns = table.columns ?? [];
  const schemaName = block.source?.schema;
  const schema = schemas.find((s) => s.name === schemaName);
  const availableFields =
    schema?.columns.map((column) => column.name) ?? [];

  const updateTable = (
    updater: (current: NonNullable<ReportBlockDefinition['table']>) => NonNullable<
      ReportBlockDefinition['table']
    >
  ) => onChange({ ...block, table: updater(table) });

  const updateColumns = (next: ReportTableColumn[]) =>
    updateTable((t) => ({ ...t, columns: next }));

  const addColumn = () => {
    const field = availableFields.find(
      (f) => !columns.some((c) => c.field === f)
    );
    if (!field) return;
    updateColumns([
      ...columns,
      { field, label: humanizeFieldName(field), type: 'value' },
    ]);
  };

  return (
    <div className="grid gap-3">
      <div className="grid gap-1.5">
        <div className="flex items-center justify-between">
          <Label className="text-xs">Columns</Label>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="h-7"
            onClick={addColumn}
            disabled={availableFields.length === 0}
          >
            <Plus className="mr-1 h-3 w-3" /> Add column
          </Button>
        </div>
        {columns.length === 0 ? (
          <p className="text-xs text-muted-foreground">
            No columns yet. Pick a schema, then add columns.
          </p>
        ) : (
          <div className="grid gap-2">
            {columns.map((column, index) => {
              const type: ReportTableColumnType = column.type ?? 'value';
              return (
                <div
                  key={`${column.field}_${index}`}
                  className="grid gap-2 rounded border p-2"
                >
                  <div className="grid grid-cols-[1fr_1fr_120px_120px_minmax(0,auto)] items-center gap-2">
                    <Select
                      value={column.field}
                      onValueChange={(value) =>
                        updateColumns(
                          columns.map((c, i) =>
                            i === index ? { ...c, field: value } : c
                          )
                        )
                      }
                    >
                      <SelectTrigger className="h-8 text-xs">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {availableFields.length === 0 ? (
                          <SelectItem disabled value={column.field}>
                            {column.field}
                          </SelectItem>
                        ) : null}
                        {availableFields.map((field) => (
                          <SelectItem key={field} value={field}>
                            {field}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                    <Input
                      value={column.label ?? ''}
                      placeholder="Label"
                      className="h-8 text-xs"
                      onChange={(event) =>
                        updateColumns(
                          columns.map((c, i) =>
                            i === index
                              ? { ...c, label: event.target.value || null }
                              : c
                          )
                        )
                      }
                    />
                    <Select
                      value={type}
                      onValueChange={(value) =>
                        updateColumns(
                          columns.map((c, i) => {
                            if (i !== index) return c;
                            const nextType = value as ReportTableColumnType;
                            if (nextType === 'workflow_button') {
                              return {
                                ...c,
                                type: nextType,
                                workflowAction:
                                  c.workflowAction ??
                                  createDefaultWorkflowAction('row'),
                              };
                            }
                            if (nextType === 'interaction_buttons') {
                              return {
                                ...c,
                                type: nextType,
                                interactionButtons:
                                  c.interactionButtons ?? [
                                    createDefaultInteractionButton(),
                                  ],
                              };
                            }
                            return { ...c, type: nextType };
                          })
                        )
                      }
                    >
                      <SelectTrigger className="h-8 text-xs">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {COLUMN_TYPES.map((option) => (
                          <SelectItem key={option.value} value={option.value}>
                            {option.label}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                    <Select
                      value={column.format ?? PLAIN}
                      onValueChange={(value) =>
                        updateColumns(
                          columns.map((c, i) =>
                            i === index
                              ? { ...c, format: value === PLAIN ? null : value }
                              : c
                          )
                        )
                      }
                    >
                      <SelectTrigger className="h-8 text-xs">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {FORMATS.map((option) => (
                          <SelectItem key={option.value} value={option.value}>
                            {option.label}
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
                        updateColumns(columns.filter((_, i) => i !== index))
                      }
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </Button>
                  </div>
                  {type === 'workflow_button' && column.workflowAction ? (
                    <WorkflowActionEditor
                      action={column.workflowAction}
                      fields={availableFields}
                      onChange={(action) =>
                        updateColumns(
                          columns.map((c, i) =>
                            i === index
                              ? { ...c, workflowAction: action }
                              : c
                          )
                        )
                      }
                    />
                  ) : null}
                  {type === 'interaction_buttons' ? (
                    <InteractionButtonsEditor
                      buttons={column.interactionButtons ?? []}
                      fields={availableFields}
                      onChange={(buttons) =>
                        updateColumns(
                          columns.map((c, i) =>
                            i === index
                              ? { ...c, interactionButtons: buttons }
                              : c
                          )
                        )
                      }
                    />
                  ) : null}
                </div>
              );
            })}
          </div>
        )}
      </div>

      <div className="grid grid-cols-2 gap-3">
        <div className="grid gap-1.5">
          <Label className="text-xs">Default page size</Label>
          <Input
            type="number"
            min={1}
            value={table.pagination?.defaultPageSize ?? ''}
            onChange={(event) => {
              const next = event.target.value
                ? Math.max(1, parseInt(event.target.value, 10))
                : null;
              updateTable((t) => ({
                ...t,
                pagination: next
                  ? { ...(t.pagination ?? {}), defaultPageSize: next }
                  : undefined,
              }));
            }}
          />
        </div>
      </div>

      <details className="rounded border p-2">
        <summary className="cursor-pointer text-xs">
          Bulk actions ({(table.actions ?? []).length})
        </summary>
        <div className="mt-2">
          <TableBulkActionsEditor
            actions={table.actions ?? []}
            fields={availableFields}
            onChange={(actions) => {
              updateTable((t) => {
                if (actions.length === 0) {
                  const rest = { ...t };
                  delete (rest as { actions?: unknown }).actions;
                  return rest;
                }
                return { ...t, actions };
              });
            }}
          />
          <div className="mt-1">
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-7"
              onClick={() =>
                updateTable((t) => ({
                  ...t,
                  actions: [
                    ...(t.actions ?? []),
                    createDefaultTableAction(),
                  ],
                }))
              }
            >
              <Plus className="mr-1 h-3 w-3" /> Add bulk action
            </Button>
          </div>
        </div>
      </details>

      <details className="rounded border p-2">
        <summary className="cursor-pointer text-xs">
          Row interactions ({(block.interactions ?? []).length})
        </summary>
        <div className="mt-2 grid gap-2">
          {(block.interactions ?? []).map((interaction, index) => (
            <div
              key={interaction.id || index}
              className="grid gap-2 rounded border p-2"
            >
              <div className="grid grid-cols-[1fr_1fr_minmax(0,auto)] items-center gap-2">
                <Input
                  value={interaction.id || ''}
                  placeholder="ID"
                  className="h-8 text-xs"
                  onChange={(event) =>
                    onChange({
                      ...block,
                      interactions: (block.interactions ?? []).map((i, idx) =>
                        idx === index
                          ? { ...i, id: event.target.value }
                          : i
                      ),
                    })
                  }
                />
                <Select
                  value={interaction.trigger?.event ?? 'row_click'}
                  onValueChange={(value) =>
                    onChange({
                      ...block,
                      interactions: (block.interactions ?? []).map((i, idx) =>
                        idx === index
                          ? {
                              ...i,
                              trigger: { ...(i.trigger ?? {}), event: value },
                            }
                          : i
                      ),
                    })
                  }
                >
                  <SelectTrigger className="h-8 text-xs">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    {['row_click', 'cell_click', 'point_click'].map((opt) => (
                      <SelectItem key={opt} value={opt}>
                        {opt}
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
                      ...block,
                      interactions: (block.interactions ?? []).filter(
                        (_, idx) => idx !== index
                      ),
                    })
                  }
                >
                  <Trash2 className="h-3.5 w-3.5" />
                </Button>
              </div>
              <InteractionActionsList
                actions={interaction.actions ?? []}
                fields={availableFields}
                onChange={(actions) =>
                  onChange({
                    ...block,
                    interactions: (block.interactions ?? []).map((i, idx) =>
                      idx === index ? { ...i, actions } : i
                    ),
                  })
                }
              />
            </div>
          ))}
          <div>
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-7"
              onClick={() =>
                onChange({
                  ...block,
                  interactions: [
                    ...(block.interactions ?? []),
                    {
                      id: `interaction_${Math.random()
                        .toString(36)
                        .slice(2, 7)}`,
                      trigger: { event: 'row_click' },
                      actions: [],
                    },
                  ],
                })
              }
            >
              <Plus className="mr-1 h-3 w-3" /> Add interaction
            </Button>
          </div>
        </div>
      </details>
    </div>
  );
}
