import { useState } from 'react';
import { ChevronDown, ChevronRight, Loader2, Pencil, Play } from 'lucide-react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';

import { Badge } from '@/shared/components/ui/badge';
import { Button } from '@/shared/components/ui/button';

import {
  ReportBlockDefinition,
  ReportBlockResult,
  ReportCardConfig,
  ReportCardField,
  ReportCardGroup,
  ReportSubtableColumn,
  ReportSubtableConfig,
  ReportWorkflowActionConfig,
} from '../../types';
import {
  formatCellValue,
  humanizeFieldName,
  isWorkflowActionDisabled,
  isWorkflowActionVisible,
} from '../../utils';
import { FieldEditor } from './editable/FieldEditor';
import { useReportWriteback } from './editable/useReportWriteback';
import { useReportWorkflowAction } from './useReportWorkflowAction';

type BadgeVariant =
  | 'default'
  | 'destructive'
  | 'outline'
  | 'secondary'
  | 'muted'
  | 'success'
  | 'warning';

const BADGE_VARIANTS: ReadonlySet<BadgeVariant> = new Set([
  'default',
  'destructive',
  'outline',
  'secondary',
  'muted',
  'success',
  'warning',
]);

function asBadgeVariant(value: string | undefined): BadgeVariant {
  return value && BADGE_VARIANTS.has(value as BadgeVariant)
    ? (value as BadgeVariant)
    : 'default';
}

type CardData = {
  row?: Record<string, unknown> | null;
  missing?: boolean;
  unsatisfiedFilter?: string;
  message?: string;
};

export function CardBlock({
  reportId,
  block,
  result,
  filters,
  blockFilters,
  onRefresh,
}: {
  reportId: string;
  block: ReportBlockDefinition;
  result: ReportBlockResult;
  filters: Record<string, unknown>;
  blockFilters: Record<string, unknown>;
  onRefresh?: () => void | Promise<void>;
}) {
  const data = (result.data ?? {}) as CardData;
  const groups = block.card?.groups ?? [];
  const writeback = useReportWriteback(reportId);
  const workflowAction = useReportWorkflowAction({ onCompleted: onRefresh });
  const [editingField, setEditingField] = useState<string | null>(null);

  if (data.missing || !data.row) {
    const fallback = data.unsatisfiedFilter
      ? `Required filter '${data.unsatisfiedFilter}' is not set.`
      : 'No record matches the current filters.';
    return (
      <div className="rounded-lg border border-dashed bg-muted/20 p-6 text-sm text-muted-foreground">
        {data.message ?? fallback}
      </div>
    );
  }

  if (groups.length === 0) {
    return (
      <div className="rounded-lg border border-destructive/30 bg-destructive/5 p-4 text-sm text-destructive">
        Card block is missing a `groups` configuration.
      </div>
    );
  }

  return (
    <CardGroups
      groups={groups}
      row={data.row}
      editingField={editingField}
      onEditField={setEditingField}
      onCommitField={(field, value, refreshAfterCommit) => {
        const ctx = getCardWritebackContext(data.row);
        if (ctx) {
          writeback.mutate(
            {
              schemaId: ctx.schemaId,
              instanceId: ctx.instanceId,
              field,
              value,
            },
            {
              onSuccess: () => {
                if (refreshAfterCommit) {
                  void onRefresh?.();
                }
              },
            }
          );
        }
        setEditingField(null);
      }}
      onCancelField={() => setEditingField(null)}
      busy={writeback.isPending}
      reportId={reportId}
      blockId={block.id}
      filters={filters}
      blockFilters={blockFilters}
      onRunWorkflow={workflowAction.run}
      isWorkflowRunning={workflowAction.isRunning}
    />
  );
}

function getCardWritebackContext(
  row: Record<string, unknown> | null | undefined
): { schemaId: string; instanceId: string } | null {
  if (!row) return null;
  const id = row.id;
  const schemaId = row.schemaId;
  if (typeof id !== 'string' || typeof schemaId !== 'string') return null;
  return { schemaId, instanceId: id };
}

type FieldEditingProps = {
  editingField?: string | null;
  onEditField?: (field: string | null) => void;
  onCommitField?: (
    field: string,
    value: unknown,
    refreshAfterCommit: boolean
  ) => void;
  onCancelField?: () => void;
  busy?: boolean;
  reportId?: string;
  blockId?: string;
  filters?: Record<string, unknown>;
  blockFilters?: Record<string, unknown>;
  onRunWorkflow?: (args: {
    key: string;
    action: ReportWorkflowActionConfig;
    row: Record<string, unknown>;
    value: unknown;
    fallbackField: string;
  }) => void | Promise<void>;
  isWorkflowRunning?: (key: string) => boolean;
  /** True when the rendered row carries the id+schemaId needed for writeback. */
  rowEditable?: boolean;
};

function CardGroups({
  groups,
  row,
  editingField,
  onEditField,
  onCommitField,
  onCancelField,
  busy,
  reportId,
  blockId,
  filters,
  blockFilters,
  onRunWorkflow,
  isWorkflowRunning,
}: {
  groups: ReportCardGroup[];
  row: Record<string, unknown>;
} & FieldEditingProps) {
  const rowEditable = (() => {
    const id = row.id;
    const schemaId = row.schemaId;
    return typeof id === 'string' && typeof schemaId === 'string';
  })();
  return (
    <div className="space-y-4">
      {groups.map((group) => (
        <CardGroup
          key={group.id}
          group={group}
          row={row}
          editingField={editingField}
          onEditField={onEditField}
          onCommitField={onCommitField}
          onCancelField={onCancelField}
          busy={busy}
          reportId={reportId}
          blockId={blockId}
          filters={filters}
          blockFilters={blockFilters}
          onRunWorkflow={onRunWorkflow}
          isWorkflowRunning={isWorkflowRunning}
          rowEditable={rowEditable}
        />
      ))}
    </div>
  );
}

function CardGroup({
  group,
  row,
  editingField,
  onEditField,
  onCommitField,
  onCancelField,
  busy,
  rowEditable,
  reportId,
  blockId,
  filters,
  blockFilters,
  onRunWorkflow,
  isWorkflowRunning,
}: {
  group: ReportCardGroup;
  row: Record<string, unknown>;
} & FieldEditingProps) {
  const columns = clampColumns(group.columns ?? 2);
  return (
    <section className="rounded-lg border bg-background">
      {(group.title || group.description) && (
        <header className="border-b px-4 py-3">
          {group.title && (
            <h3 className="text-sm font-semibold text-foreground">
              {group.title}
            </h3>
          )}
          {group.description && (
            <p className="mt-0.5 text-xs text-muted-foreground">
              {group.description}
            </p>
          )}
        </header>
      )}
      <div
        className="grid gap-x-6 gap-y-4 px-4 py-4"
        style={{ gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` }}
      >
        {group.fields.map((field) => (
          <CardField
            key={field.field}
            field={field}
            row={row}
            maxColumns={columns}
            editingField={editingField}
            onEditField={onEditField}
            onCommitField={onCommitField}
            onCancelField={onCancelField}
            busy={busy}
            reportId={reportId}
            blockId={blockId}
            filters={filters}
            blockFilters={blockFilters}
            onRunWorkflow={onRunWorkflow}
            isWorkflowRunning={isWorkflowRunning}
            rowEditable={rowEditable}
          />
        ))}
      </div>
    </section>
  );
}

function CardField({
  field,
  row,
  maxColumns,
  editingField,
  onEditField,
  onCommitField,
  onCancelField,
  busy,
  rowEditable,
  reportId,
  blockId,
  filters,
  blockFilters,
  onRunWorkflow,
  isWorkflowRunning,
}: {
  field: ReportCardField;
  row: Record<string, unknown>;
  maxColumns: number;
} & FieldEditingProps) {
  const span = Math.min(Math.max(field.colSpan ?? 1, 1), maxColumns);
  const label = field.label ?? humanizeFieldName(field.field);
  const value = row[field.field];
  const displayValue = getDisplayValue(row, field.displayField, value);
  const kind = field.kind ?? 'value';
  const actionKey = `${blockId ?? 'card'}:${String(row.id ?? '')}:${field.field}`;
  const hasWorkflowAction =
    kind === 'workflow_button' || field.workflowAction !== undefined;
  if (
    hasWorkflowAction &&
    field.workflowAction &&
    !isWorkflowActionVisible(field.workflowAction, row)
  ) {
    return null;
  }
  const canEdit = Boolean(field.editable && rowEditable && kind === 'value');
  const isEditing = canEdit && editingField === field.field;

  return (
    <div
      className="group/field min-w-0"
      style={{ gridColumn: `span ${span} / span ${span}` }}
    >
      <div className="flex items-center gap-1">
        <p className="text-xs font-medium uppercase tracking-wide text-muted-foreground">
          {label}
        </p>
        {canEdit && !isEditing && (
          <button
            type="button"
            aria-label="Edit field"
            className="rounded p-0.5 text-muted-foreground opacity-0 transition-opacity hover:bg-muted hover:text-foreground group-hover/field:opacity-100"
            onClick={() => onEditField?.(field.field)}
          >
            <Pencil className="h-3 w-3" />
          </button>
        )}
      </div>
      <div className="mt-1 text-sm text-foreground">
        {isEditing ? (
          <FieldEditor
            value={value}
            displayValue={displayValue}
            format={field.format}
            pillVariants={field.pillVariants}
            editor={field.editor}
            lookupContext={
              reportId && blockId
                ? {
                    reportId,
                    blockId,
                    field: field.field,
                    filters: filters ?? {},
                    blockFilters: blockFilters ?? {},
                  }
                : undefined
            }
            busy={busy}
            onCommit={(next) =>
              onCommitField?.(
                field.field,
                next,
                shouldRefreshAfterWriteback(field)
              )
            }
            onCancel={() => onCancelField?.()}
          />
        ) : kind === 'json' ? (
          <JsonField value={value} collapsed={field.collapsed ?? true} />
        ) : kind === 'markdown' ? (
          <MarkdownField value={value} collapsed={field.collapsed ?? false} />
        ) : kind === 'subcard' ? (
          <SubcardField
            value={value}
            config={field.subcard}
            collapsed={field.collapsed ?? false}
          />
        ) : kind === 'subtable' ? (
          <SubtableField
            value={value}
            config={field.subtable}
            collapsed={field.collapsed ?? false}
          />
        ) : hasWorkflowAction && field.workflowAction ? (
          <WorkflowActionButton
            action={field.workflowAction}
            labelFallback={field.label ?? 'Run'}
            running={isWorkflowRunning?.(actionKey) ?? false}
            disabled={isWorkflowActionDisabled(field.workflowAction, row)}
            value={value}
            row={row}
            fallbackField={field.field}
            actionKey={actionKey}
            onRun={onRunWorkflow}
          />
        ) : (
          <ValueField field={field} value={displayValue} />
        )}
      </div>
    </div>
  );
}

function getDisplayValue(
  row: Record<string, unknown>,
  displayField: string | undefined,
  value: unknown
) {
  if (!displayField) return value;
  const displayValue = row[displayField];
  if (displayValue === null || displayValue === undefined) return value;
  if (typeof displayValue === 'string' && displayValue.trim().length === 0) {
    return value;
  }
  return displayValue;
}

function shouldRefreshAfterWriteback(field: ReportCardField): boolean {
  return Boolean(field.displayField || field.editor?.kind === 'lookup');
}

function WorkflowActionButton({
  action,
  labelFallback,
  running,
  disabled,
  value,
  row,
  fallbackField,
  actionKey,
  onRun,
}: {
  action: ReportWorkflowActionConfig;
  labelFallback: string;
  running: boolean;
  disabled: boolean;
  value: unknown;
  row: Record<string, unknown>;
  fallbackField: string;
  actionKey: string;
  onRun?: (args: {
    key: string;
    action: ReportWorkflowActionConfig;
    row: Record<string, unknown>;
    value: unknown;
    fallbackField: string;
  }) => void | Promise<void>;
}) {
  return (
    <Button
      type="button"
      variant="outline"
      size="sm"
      className="h-8 max-w-full gap-1.5"
      disabled={running || disabled || !onRun}
      onClick={() => {
        if (disabled) return;
        void onRun?.({
          key: actionKey,
          action,
          row,
          value,
          fallbackField,
        });
      }}
    >
      {running ? (
        <Loader2 className="h-3.5 w-3.5 animate-spin" />
      ) : (
        <Play className="h-3.5 w-3.5" />
      )}
      <span className="truncate">
        {running
          ? (action.runningLabel ?? 'Running...')
          : (action.label ?? labelFallback)}
      </span>
    </Button>
  );
}

function SubcardField({
  value,
  config,
  collapsed,
}: {
  value: unknown;
  config?: ReportCardConfig;
  collapsed: boolean;
}) {
  const [open, setOpen] = useState(!collapsed);

  if (!config || config.groups.length === 0) {
    return (
      <span className="text-xs text-destructive">Missing subcard config.</span>
    );
  }

  if (value === null || value === undefined) {
    return <span className="text-muted-foreground">—</span>;
  }
  if (typeof value !== 'object' || Array.isArray(value)) {
    return (
      <span className="text-xs text-destructive">
        Subcard expects an object value, got{' '}
        {Array.isArray(value) ? 'array' : typeof value}.
      </span>
    );
  }

  const inner = value as Record<string, unknown>;
  const body = <CardGroups groups={config.groups} row={inner} />;

  if (!collapsed) return body;
  return (
    <div className="space-y-2">
      <CollapseToggle open={open} onToggle={() => setOpen((p) => !p)} />
      {open && body}
    </div>
  );
}

function SubtableField({
  value,
  config,
  collapsed,
}: {
  value: unknown;
  config?: ReportSubtableConfig;
  collapsed: boolean;
}) {
  const [open, setOpen] = useState(!collapsed);

  if (!config || config.columns.length === 0) {
    return (
      <span className="text-xs text-destructive">Missing subtable config.</span>
    );
  }

  if (value === null || value === undefined) {
    return <span className="text-muted-foreground">—</span>;
  }

  let rows: Array<Record<string, unknown>>;
  if (Array.isArray(value)) {
    rows = value as Array<Record<string, unknown>>;
  } else if (typeof value === 'string') {
    try {
      const parsed = JSON.parse(value);
      if (!Array.isArray(parsed)) {
        return (
          <span className="text-xs text-destructive">
            Subtable expects a JSON array, got {typeof parsed}.
          </span>
        );
      }
      rows = parsed as Array<Record<string, unknown>>;
    } catch {
      return (
        <span className="text-xs text-destructive">
          Subtable received non-JSON string value.
        </span>
      );
    }
  } else {
    return (
      <span className="text-xs text-destructive">
        Subtable expects an array, got {typeof value}.
      </span>
    );
  }

  if (rows.length === 0) {
    return (
      <span className="text-muted-foreground">
        {config.emptyLabel ?? 'No items.'}
      </span>
    );
  }

  const table = (
    <div className="overflow-hidden rounded-md border">
      <table className="w-full text-left text-xs">
        <thead className="bg-muted/40">
          <tr>
            {config.columns.map((column) => (
              <th
                key={column.field}
                className={`px-3 py-2 font-medium text-muted-foreground ${alignClass(column.align)}`}
              >
                {column.label ?? humanizeFieldName(column.field)}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, index) => (
            <tr
              key={(row.id as string | undefined) ?? index}
              className="border-t"
            >
              {config.columns.map((column) => (
                <td
                  key={column.field}
                  className={`px-3 py-2 align-top ${alignClass(column.align)}`}
                >
                  <SubtableCell column={column} value={row[column.field]} />
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );

  if (!collapsed) return table;
  return (
    <div className="space-y-2">
      <CollapseToggle
        open={open}
        onToggle={() => setOpen((p) => !p)}
        summary={`${rows.length} ${rows.length === 1 ? 'row' : 'rows'}`}
      />
      {open && table}
    </div>
  );
}

function SubtableCell({
  column,
  value,
}: {
  column: ReportSubtableColumn;
  value: unknown;
}) {
  if (value === null || value === undefined || value === '') {
    return <span className="text-muted-foreground">—</span>;
  }
  if (column.format === 'pill') {
    const key = pillKey(value);
    if (key !== null) {
      return (
        <Badge
          variant={asBadgeVariant(column.pillVariants?.[key])}
          className="rounded-full px-2 py-0.5 text-[10px]"
        >
          {humanizePillLabel(value)}
        </Badge>
      );
    }
  }
  if (typeof value === 'boolean') return <>{value ? 'Yes' : 'No'}</>;
  if (typeof value === 'object') {
    return (
      <code className="rounded bg-muted/50 px-1 py-0.5 text-[10px]">
        {JSON.stringify(value)}
      </code>
    );
  }
  return <>{formatCellValue(value, column.format ?? undefined)}</>;
}

function pillKey(value: unknown): string | null {
  if (typeof value === 'string' && value.length > 0) return value;
  if (typeof value === 'boolean') return String(value);
  if (typeof value === 'number') return String(value);
  return null;
}

function humanizePillLabel(value: unknown): string {
  if (typeof value === 'boolean') return value ? 'Yes' : 'No';
  if (typeof value === 'string') return humanizeFieldName(value);
  return String(value ?? '');
}

function alignClass(align?: string) {
  if (align === 'right') return 'text-right';
  if (align === 'center') return 'text-center';
  return 'text-left';
}

function CollapseToggle({
  open,
  onToggle,
  summary,
}: {
  open: boolean;
  onToggle: () => void;
  summary?: string;
}) {
  return (
    <button
      type="button"
      onClick={onToggle}
      className="inline-flex items-center gap-1 rounded text-xs font-medium text-muted-foreground hover:text-foreground"
    >
      {open ? (
        <ChevronDown className="h-3 w-3" />
      ) : (
        <ChevronRight className="h-3 w-3" />
      )}
      <span>{open ? 'Hide' : 'Show'}</span>
      {!open && summary && (
        <span className="ml-1 text-muted-foreground">{summary}</span>
      )}
    </button>
  );
}

function ValueField({
  field,
  value,
}: {
  field: ReportCardField;
  value: unknown;
}) {
  if (value === null || value === undefined || value === '') {
    return <span className="text-muted-foreground">—</span>;
  }

  if (field.format === 'pill') {
    const key = pillKey(value);
    if (key !== null) {
      return (
        <Badge
          variant={asBadgeVariant(field.pillVariants?.[key])}
          className="rounded-full px-2.5 py-0.5"
        >
          {humanizePillLabel(value)}
        </Badge>
      );
    }
  }

  if (typeof value === 'boolean') {
    return value ? 'Yes' : 'No';
  }

  return <span>{formatCellValue(value, field.format ?? undefined)}</span>;
}

function JsonField({
  value,
  collapsed,
}: {
  value: unknown;
  collapsed: boolean;
}) {
  const [open, setOpen] = useState(!collapsed);

  if (value === null || value === undefined) {
    return <span className="text-muted-foreground">—</span>;
  }

  let pretty: string;
  try {
    pretty =
      typeof value === 'string'
        ? prettyMaybeJsonString(value)
        : JSON.stringify(value, null, 2);
  } catch {
    pretty = String(value);
  }

  const summary = jsonSummary(value);

  return (
    <div className="space-y-1">
      <button
        type="button"
        onClick={() => setOpen((prev) => !prev)}
        className="inline-flex items-center gap-1 rounded text-xs font-medium text-muted-foreground hover:text-foreground"
      >
        {open ? (
          <ChevronDown className="h-3 w-3" />
        ) : (
          <ChevronRight className="h-3 w-3" />
        )}
        <span>{open ? 'Hide' : 'Show'}</span>
        {!open && summary && (
          <span className="ml-1 text-muted-foreground">{summary}</span>
        )}
      </button>
      {open && (
        <pre className="max-h-96 overflow-auto rounded-md border bg-muted/40 p-3 text-xs leading-relaxed text-foreground">
          {pretty}
        </pre>
      )}
    </div>
  );
}

function MarkdownField({
  value,
  collapsed,
}: {
  value: unknown;
  collapsed: boolean;
}) {
  const [open, setOpen] = useState(!collapsed);

  if (value === null || value === undefined || value === '') {
    return <span className="text-muted-foreground">—</span>;
  }

  const content = typeof value === 'string' ? value : JSON.stringify(value);

  if (!collapsed) {
    return (
      <div className="prose prose-sm prose-slate max-w-none dark:prose-invert">
        <ReactMarkdown remarkPlugins={[remarkGfm]}>{content}</ReactMarkdown>
      </div>
    );
  }

  return (
    <div className="space-y-1">
      <button
        type="button"
        onClick={() => setOpen((prev) => !prev)}
        className="inline-flex items-center gap-1 rounded text-xs font-medium text-muted-foreground hover:text-foreground"
      >
        {open ? (
          <ChevronDown className="h-3 w-3" />
        ) : (
          <ChevronRight className="h-3 w-3" />
        )}
        <span>{open ? 'Hide' : 'Show'}</span>
      </button>
      {open && (
        <div className="prose prose-sm prose-slate max-w-none dark:prose-invert">
          <ReactMarkdown remarkPlugins={[remarkGfm]}>{content}</ReactMarkdown>
        </div>
      )}
    </div>
  );
}

function clampColumns(value: number): number {
  if (!Number.isFinite(value)) return 2;
  return Math.min(Math.max(Math.floor(value), 1), 4);
}

function prettyMaybeJsonString(raw: string): string {
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}

function jsonSummary(value: unknown): string {
  if (Array.isArray(value)) return `${value.length} items`;
  if (value && typeof value === 'object') {
    const keys = Object.keys(value);
    if (keys.length === 0) return 'empty object';
    return `${keys.length} fields`;
  }
  return '';
}
