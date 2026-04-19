import { useMemo, useState, useEffect } from 'react';
import { Schema } from '@/generated/RuntaraRuntimeApi';
import { Checkbox } from '@/shared/components/ui/checkbox';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import { Switch } from '@/shared/components/ui/switch';
import { Button } from '@/shared/components/ui/button';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/shared/components/ui/alert-dialog';

type ColumnDataType =
  | 'string'
  | 'integer'
  | 'boolean'
  | 'decimal'
  | 'timestamp'
  | 'json'
  | 'enum';

// Mirrors mapping used in ObjectInstanceForm.
function mapPostgresTypeToDataType(pgType: string): ColumnDataType {
  const baseType = pgType.replace(/\[\]$/, '');
  if (baseType.startsWith('VARCHAR') || baseType === 'TEXT') return 'string';
  if (
    baseType === 'INTEGER' ||
    baseType === 'BIGINT' ||
    baseType === 'SMALLINT'
  )
    return 'integer';
  if (baseType.startsWith('DECIMAL') || baseType.startsWith('NUMERIC'))
    return 'decimal';
  if (baseType === 'BOOLEAN') return 'boolean';
  if (
    baseType === 'DATE' ||
    baseType === 'TIMESTAMP' ||
    baseType === 'TIMESTAMPTZ'
  )
    return 'timestamp';
  if (baseType === 'JSONB' || baseType === 'JSON') return 'json';
  return 'string';
}

function coerceValue(raw: unknown, dataType: ColumnDataType): unknown {
  switch (dataType) {
    case 'integer':
      return typeof raw === 'number' ? raw : parseInt(String(raw), 10) || 0;
    case 'decimal':
      return typeof raw === 'number' ? raw : parseFloat(String(raw)) || 0;
    case 'boolean':
      return !!raw;
    case 'json':
      if (typeof raw === 'string') {
        try {
          return JSON.parse(raw);
        } catch {
          return raw;
        }
      }
      return raw;
    default:
      return raw;
  }
}

interface BulkEditDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  selectedCount: number;
  schema: Schema;
  onSubmit: (properties: Record<string, unknown>) => Promise<void>;
  isSubmitting?: boolean;
}

/**
 * Dialog that lets the user pick one or more schema columns and new values to
 * apply to every selected row. Backed by PATCH /instances/{schema_id}/bulk
 * with mode=byCondition (IN("id", [...selected])).
 */
export function BulkEditDialog({
  open,
  onOpenChange,
  selectedCount,
  schema,
  onSubmit,
  isSubmitting,
}: BulkEditDialogProps) {
  const columns = useMemo(() => schema.columns ?? [], [schema]);
  const [pickedFields, setPickedFields] = useState<Set<string>>(new Set());
  const [values, setValues] = useState<Record<string, unknown>>({});

  // Reset when dialog closes so a subsequent open starts clean.
  useEffect(() => {
    if (!open) {
      setPickedFields(new Set());
      setValues({});
    }
  }, [open]);

  const togglePicked = (name: string, picked: boolean) => {
    setPickedFields((prev) => {
      const next = new Set(prev);
      if (picked) next.add(name);
      else next.delete(name);
      return next;
    });
    if (!picked) {
      setValues((prev) => {
        const copy = { ...prev };
        delete copy[name];
        return copy;
      });
    }
  };

  const setFieldValue = (name: string, value: unknown) =>
    setValues((prev) => ({ ...prev, [name]: value }));

  const handleConfirm = async () => {
    const columnTypeMap = new Map<string, ColumnDataType>();
    columns.forEach((c) => columnTypeMap.set(c.name, mapPostgresTypeToDataType(c.type)));

    const properties: Record<string, unknown> = {};
    for (const name of pickedFields) {
      const dt = columnTypeMap.get(name);
      if (!dt) continue;
      properties[name] = coerceValue(values[name], dt);
    }
    await onSubmit(properties);
  };

  const canSubmit = pickedFields.size > 0 && !isSubmitting;

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent className="max-w-lg">
        <AlertDialogHeader>
          <AlertDialogTitle>Edit {selectedCount} selected</AlertDialogTitle>
          <AlertDialogDescription>
            Choose which fields to update. The new value will be applied to
            every selected record.
          </AlertDialogDescription>
        </AlertDialogHeader>

        <div className="max-h-[50vh] overflow-y-auto space-y-3 pr-1">
          {columns.length === 0 && (
            <p className="text-sm text-muted-foreground">
              This schema has no editable columns.
            </p>
          )}
          {columns.map((col) => {
            const dt = mapPostgresTypeToDataType(col.type);
            const picked = pickedFields.has(col.name);
            const currentValue = values[col.name];
            return (
              <div key={col.name} className="flex items-start gap-3">
                <Checkbox
                  id={`bulk-edit-field-${col.name}`}
                  checked={picked}
                  onCheckedChange={(state) =>
                    togglePicked(col.name, state === true)
                  }
                  className="mt-2"
                />
                <div className="flex-1 space-y-1">
                  <Label
                    htmlFor={`bulk-edit-field-${col.name}`}
                    className="text-sm font-medium"
                  >
                    {col.name}{' '}
                    <span className="text-xs text-muted-foreground">
                      ({dt})
                    </span>
                  </Label>
                  {picked &&
                    renderValueInput(col, dt, currentValue, (v) =>
                      setFieldValue(col.name, v)
                    )}
                </div>
              </div>
            );
          })}
        </div>

        <AlertDialogFooter>
          <AlertDialogCancel disabled={isSubmitting}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            onClick={(e) => {
              e.preventDefault();
              void handleConfirm();
            }}
            disabled={!canSubmit}
          >
            {isSubmitting ? 'Updating…' : 'Update'}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}

function renderValueInput(
  col: { name: string; type: string; enumValues?: string[] | null },
  dataType: ColumnDataType,
  value: unknown,
  onChange: (v: unknown) => void
) {
  if (dataType === 'boolean') {
    return (
      <div className="flex items-center gap-2">
        <Switch
          checked={!!value}
          onCheckedChange={(checked) => onChange(checked)}
        />
        <span className="text-sm text-muted-foreground">
          {value ? 'true' : 'false'}
        </span>
      </div>
    );
  }
  if (dataType === 'enum' && col.enumValues && col.enumValues.length > 0) {
    return (
      <Select
        value={typeof value === 'string' ? value : ''}
        onValueChange={(v) => onChange(v)}
      >
        <SelectTrigger>
          <SelectValue placeholder="Select a value" />
        </SelectTrigger>
        <SelectContent>
          {col.enumValues.map((ev) => (
            <SelectItem key={ev} value={ev}>
              {ev}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    );
  }
  if (dataType === 'json') {
    return (
      <Input
        placeholder='{"key": "value"}'
        value={typeof value === 'string' ? value : JSON.stringify(value ?? {})}
        onChange={(e) => onChange(e.target.value)}
      />
    );
  }
  if (dataType === 'timestamp') {
    return (
      <Input
        type="datetime-local"
        value={typeof value === 'string' ? value : ''}
        onChange={(e) => onChange(e.target.value)}
      />
    );
  }
  return (
    <Input
      type={dataType === 'integer' || dataType === 'decimal' ? 'number' : 'text'}
      step={dataType === 'decimal' ? 'any' : undefined}
      value={value === undefined || value === null ? '' : String(value)}
      onChange={(e) => onChange(e.target.value)}
    />
  );
}
