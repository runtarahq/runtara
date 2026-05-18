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
  ReportCardConfig,
  ReportCardField,
  ReportCardFieldKind,
  ReportCardGroup,
} from '../../../types';

interface CardBlockEditorProps {
  block: ReportBlockDefinition;
  schemas: Schema[];
  onChange: (block: ReportBlockDefinition) => void;
}

const FIELD_KINDS: Array<{ value: ReportCardFieldKind; label: string }> = [
  { value: 'value', label: 'Value' },
  { value: 'json', label: 'JSON' },
  { value: 'markdown', label: 'Markdown' },
  { value: 'subcard', label: 'Subcard' },
  { value: 'subtable', label: 'Subtable' },
  { value: 'workflow_button', label: 'Workflow button' },
];

const FORMAT_PLAIN = '__plain__';
const FORMATS = [
  { value: 'number', label: 'Number' },
  { value: 'decimal', label: 'Decimal' },
  { value: 'currency', label: 'Currency' },
  { value: 'percent', label: 'Percent' },
  { value: 'bytes', label: 'Bytes (KB / MB / GB)' },
  { value: 'date', label: 'Date' },
  { value: 'datetime', label: 'Date + time' },
  { value: 'pill', label: 'Pill' },
];

function newGroup(): ReportCardGroup {
  return {
    id: `group_${Math.random().toString(36).slice(2, 7)}`,
    fields: [],
  };
}

function newField(field: string): ReportCardField {
  return { field, kind: 'value' };
}

export function CardBlockEditor({
  block,
  schemas,
  onChange,
}: CardBlockEditorProps) {
  const card: ReportCardConfig = block.card ?? { groups: [] };
  const groups: ReportCardGroup[] = card.groups ?? [];
  const schema = schemas.find((s) => s.name === block.source?.schema);
  const availableFields = schema?.columns.map((c) => c.name) ?? [];

  const updateGroups = (next: ReportCardGroup[]) =>
    onChange({ ...block, card: { ...card, groups: next } });

  return (
    <div className="grid gap-3">
      {groups.length === 0 ? (
        <p className="text-xs text-muted-foreground">
          No groups yet. A card has one or more groups, each with its own
          column layout and fields.
        </p>
      ) : (
        <div className="grid gap-3">
          {groups.map((group, gIndex) => (
            <GroupEditor
              key={group.id}
              group={group}
              availableFields={availableFields}
              onChange={(updated) =>
                updateGroups(
                  groups.map((g, i) => (i === gIndex ? updated : g))
                )
              }
              onDelete={() =>
                updateGroups(groups.filter((_, i) => i !== gIndex))
              }
            />
          ))}
        </div>
      )}
      <div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-7"
          onClick={() => updateGroups([...groups, newGroup()])}
        >
          <Plus className="mr-1 h-3 w-3" /> Add group
        </Button>
      </div>
    </div>
  );
}

interface GroupEditorProps {
  group: ReportCardGroup;
  availableFields: string[];
  onChange: (group: ReportCardGroup) => void;
  onDelete: () => void;
}

function GroupEditor({
  group,
  availableFields,
  onChange,
  onDelete,
}: GroupEditorProps) {
  const fields = group.fields ?? [];

  const updateFields = (next: ReportCardField[]) =>
    onChange({ ...group, fields: next });

  return (
    <div className="grid gap-2 rounded border p-2">
      <div className="grid grid-cols-[1fr_120px_minmax(0,auto)] items-center gap-2">
        <Input
          value={group.title ?? ''}
          placeholder="Group title (optional)"
          className="h-8 text-xs"
          onChange={(event) =>
            onChange({ ...group, title: event.target.value || null })
          }
        />
        <Input
          type="number"
          min={1}
          max={4}
          value={group.columns ?? ''}
          placeholder="Cols"
          className="h-8 text-xs"
          onChange={(event) => {
            const next = event.target.value
              ? Math.max(1, parseInt(event.target.value, 10))
              : undefined;
            onChange({ ...group, columns: next });
          }}
        />
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-8 w-8 text-destructive"
          onClick={onDelete}
        >
          <Trash2 className="h-3.5 w-3.5" />
        </Button>
      </div>
      <Input
        value={group.description ?? ''}
        placeholder="Group description (optional)"
        className="h-8 text-xs"
        onChange={(event) =>
          onChange({ ...group, description: event.target.value || null })
        }
      />
      <div className="grid gap-1.5">
        <div className="flex items-center justify-between">
          <Label className="text-xs">Fields</Label>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="h-7"
            onClick={() => {
              const used = new Set(fields.map((f) => f.field));
              const field = availableFields.find((f) => !used.has(f)) ?? '';
              updateFields([...fields, newField(field)]);
            }}
          >
            <Plus className="mr-1 h-3 w-3" /> Add field
          </Button>
        </div>
        {fields.length === 0 ? (
          <p className="text-xs text-muted-foreground">No fields yet.</p>
        ) : (
          <div className="grid gap-1.5">
            {fields.map((field, index) => (
              <FieldEditor
                key={index}
                field={field}
                availableFields={availableFields}
                onChange={(updated) =>
                  updateFields(
                    fields.map((f, i) => (i === index ? updated : f))
                  )
                }
                onDelete={() =>
                  updateFields(fields.filter((_, i) => i !== index))
                }
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

interface FieldEditorProps {
  field: ReportCardField;
  availableFields: string[];
  onChange: (field: ReportCardField) => void;
  onDelete: () => void;
}

function FieldEditor({
  field,
  availableFields,
  onChange,
  onDelete,
}: FieldEditorProps) {
  const kind: ReportCardFieldKind = field.kind ?? 'value';
  const hasNested = kind === 'subcard' || kind === 'subtable';

  return (
    <div className="grid gap-1.5 rounded border bg-muted/20 p-2">
      <div className="grid grid-cols-[1fr_1fr_120px_120px_minmax(0,auto)] items-center gap-2">
        <Select
          value={field.field || ''}
          onValueChange={(value) => onChange({ ...field, field: value })}
        >
          <SelectTrigger className="h-7 text-xs">
            <SelectValue placeholder="Field" />
          </SelectTrigger>
          <SelectContent>
            {field.field && !availableFields.includes(field.field) ? (
              <SelectItem disabled value={field.field}>
                {field.field}
              </SelectItem>
            ) : null}
            {availableFields.map((option) => (
              <SelectItem key={option} value={option}>
                {option}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Input
          value={field.label ?? ''}
          placeholder="Label"
          className="h-7 text-xs"
          onChange={(event) =>
            onChange({ ...field, label: event.target.value || null })
          }
        />
        <Select
          value={kind}
          onValueChange={(value) =>
            onChange({ ...field, kind: value as ReportCardFieldKind })
          }
        >
          <SelectTrigger className="h-7 text-xs">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {FIELD_KINDS.map((option) => (
              <SelectItem key={option.value} value={option.value}>
                {option.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select
          value={field.format ?? FORMAT_PLAIN}
          onValueChange={(value) =>
            onChange({
              ...field,
              format: value === FORMAT_PLAIN ? null : value,
            })
          }
        >
          <SelectTrigger className="h-7 text-xs">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value={FORMAT_PLAIN}>Plain</SelectItem>
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
          className="h-7 w-7 text-destructive"
          onClick={onDelete}
        >
          <Trash2 className="h-3.5 w-3.5" />
        </Button>
      </div>
      {hasNested ? (
        <p className="text-xs text-amber-700 dark:text-amber-300">
          Subcard / subtable nested config is preserved on save; edit advanced
          structure via the legacy wizard until v2 exposes the recursive form.
        </p>
      ) : null}
    </div>
  );
}
