import { useEffect, useMemo, useState } from 'react';
import { Button } from '@/shared/components/ui/button';
import { Checkbox } from '@/shared/components/ui/checkbox';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import { Plus, Settings2, Trash2 } from 'lucide-react';
import { cn } from '@/lib/utils';
import { validateSchemaFieldsWithRust } from '@/features/workflows/utils/rust-workflow-validation';
import type { RustSchemaFieldsValidationError } from '@/features/workflows/utils/rust-workflow-validation';
import {
  buildSchemaFromFields as buildSchemaObjectFromFields,
  parseSchema as parseRawSchema,
} from '@/features/workflows/utils/schema';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { Textarea } from '@/shared/components/ui/textarea';
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '@/shared/components/ui/dialog';

const SUPPORTED_TYPES: { label: string; value: string }[] = [
  { label: 'String', value: 'string' },
  { label: 'Number', value: 'number' },
  { label: 'Integer', value: 'integer' },
  { label: 'Boolean', value: 'boolean' },
  { label: 'Object', value: 'object' },
  { label: 'Array', value: 'array' },
  { label: 'File', value: 'file' },
];

export type SchemaField = {
  name: string;
  type: string;
  required: boolean;
  description: string;
  defaultValue?: any;
  enum?: any[];
  example?: any;
  items?: any;
  nullable?: boolean;
  label?: string;
  placeholder?: string;
  order?: number;
  format?: string;
  min?: number;
  max?: number;
  pattern?: string;
  properties?: SchemaField[];
  visibleWhen?: {
    field: string;
    equals?: any;
    notEquals?: any;
  };
  extensions?: Record<string, any>;
};

const ADVANCED_SCHEMA_KEYS = new Set([
  'example',
  'items',
  'label',
  'placeholder',
  'order',
  'format',
  'min',
  'max',
  'pattern',
  'properties',
  'visibleWhen',
]);

function formatFieldValue(value: any): string {
  if (value === undefined || value === null) return '';
  if (typeof value === 'string') return value;
  return JSON.stringify(value);
}

function parseDefaultValue(raw: string, type: string | undefined): any {
  if (raw.trim() === '') return undefined;

  if (type === 'number') {
    const value = Number(raw);
    return Number.isFinite(value) ? value : raw;
  }

  if (type === 'integer') {
    const value = parseInt(raw, 10);
    return Number.isFinite(value) ? value : raw;
  }

  if (type === 'boolean') {
    if (raw.toLowerCase() === 'true') return true;
    if (raw.toLowerCase() === 'false') return false;
    return raw;
  }

  if (type === 'object' || type === 'array') {
    try {
      return JSON.parse(raw);
    } catch {
      return raw;
    }
  }

  return raw;
}

function formatEnumValue(values: any[] | undefined): string {
  if (!Array.isArray(values) || values.length === 0) return '';
  const hasStructuredValue = values.some(
    (value) => value !== null && typeof value === 'object'
  );
  if (hasStructuredValue) {
    return JSON.stringify(values);
  }
  return values.map((value) => String(value)).join(', ');
}

function parseEnumValue(raw: string): any[] | undefined {
  const trimmed = raw.trim();
  if (!trimmed) return undefined;

  if (trimmed.startsWith('[')) {
    try {
      const parsed = JSON.parse(trimmed);
      return Array.isArray(parsed) ? parsed : [parsed];
    } catch {
      // Fall back to comma-separated values below.
    }
  }

  const values = raw
    .split(',')
    .map((value) => value.trim())
    .filter(Boolean);
  return values.length > 0 ? values : undefined;
}

function toAdvancedSchemaObject(field: SchemaField): Record<string, any> {
  const advanced: Record<string, any> = { ...(field.extensions || {}) };
  for (const key of ADVANCED_SCHEMA_KEYS) {
    if (key === 'properties') {
      if (field.properties && field.properties.length > 0) {
        advanced.properties = buildSchemaObjectFromFields(
          field.properties as any
        );
      }
      continue;
    }

    const value = (field as any)[key];
    if (value !== undefined && value !== '') {
      advanced[key] = value;
    }
  }
  return advanced;
}

function applyAdvancedSchemaObject(
  field: SchemaField,
  advanced: Record<string, any>
): SchemaField {
  const next: SchemaField = { ...field };

  for (const key of ADVANCED_SCHEMA_KEYS) {
    delete (next as any)[key];
  }

  const extensions: Record<string, any> = {};
  for (const [key, value] of Object.entries(advanced)) {
    if (key === 'properties') {
      next.properties = Array.isArray(value)
        ? (value as SchemaField[])
        : (parseRawSchema(value as any).map((property) => ({
            ...property,
            name: property.name,
            type: property.type || 'string',
            required: property.required !== false,
            description: property.description || '',
          })) as SchemaField[]);
      continue;
    }

    if (ADVANCED_SCHEMA_KEYS.has(key)) {
      (next as any)[key] = value;
      continue;
    }

    extensions[key] = value;
  }

  next.extensions = Object.keys(extensions).length > 0 ? extensions : undefined;
  return next;
}

function AdvancedSchemaFieldDialog({
  field,
  readOnly,
  onApply,
}: {
  field: SchemaField;
  readOnly: boolean;
  onApply: (field: SchemaField) => void;
}) {
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState('');
  const [error, setError] = useState<string | null>(null);
  const advanced = useMemo(() => toAdvancedSchemaObject(field), [field]);
  const hasAdvanced = Object.keys(advanced).length > 0;

  useEffect(() => {
    if (!open) return;
    setDraft(JSON.stringify(advanced, null, 2));
    setError(null);
  }, [advanced, open]);

  const handleApply = () => {
    try {
      const parsed = draft.trim() ? JSON.parse(draft) : {};
      if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
        setError('Advanced schema must be a JSON object.');
        return;
      }

      onApply(applyAdvancedSchemaObject(field, parsed));
      setOpen(false);
    } catch (parseError) {
      setError(
        parseError instanceof Error ? parseError.message : 'Invalid JSON.'
      );
    }
  };

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <Button
          type="button"
          variant={hasAdvanced ? 'secondary' : 'ghost'}
          size="sm"
          className="h-7 w-7 p-0"
          aria-label={`Edit advanced schema for ${field.name || 'field'}`}
        >
          <Settings2 className="h-3.5 w-3.5" />
        </Button>
      </DialogTrigger>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle>Advanced Schema</DialogTitle>
        </DialogHeader>
        <Textarea
          value={draft}
          onChange={(event) => {
            setDraft(event.target.value);
            setError(null);
          }}
          disabled={readOnly}
          className="min-h-[280px] font-mono text-xs"
          spellCheck={false}
        />
        {error && <p className="text-sm text-destructive">{error}</p>}
        <DialogFooter>
          <Button
            type="button"
            variant="outline"
            onClick={() => setOpen(false)}
          >
            Cancel
          </Button>
          <Button type="button" onClick={handleApply} disabled={readOnly}>
            Apply
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

interface SchemaFieldsEditorProps {
  label: string;
  fields: SchemaField[];
  onChange: (fields: SchemaField[]) => void;
  readOnly?: boolean;
  emptyMessage?: string;
  hideLabel?: boolean;
  showEnum?: boolean;
}

export function SchemaFieldsEditor({
  label,
  fields,
  onChange,
  readOnly = false,
  emptyMessage = 'No fields defined.',
  hideLabel = false,
  showEnum = false,
}: SchemaFieldsEditorProps) {
  const [schemaFieldValidationErrors, setSchemaFieldValidationErrors] =
    useState<RustSchemaFieldsValidationError[]>([]);

  useEffect(() => {
    let cancelled = false;

    validateSchemaFieldsWithRust(label, fields).then((result) => {
      if (cancelled) {
        return;
      }

      setSchemaFieldValidationErrors(
        result.status === 'invalid' ? result.schemaErrors : []
      );
    });

    return () => {
      cancelled = true;
    };
  }, [fields, label]);

  const fieldNameErrorsByIndex = useMemo(() => {
    const errorsByIndex = new Map<number, string>();

    for (const error of schemaFieldValidationErrors) {
      if (error.code !== 'E008') {
        continue;
      }

      for (const rowIndex of error.rowIndices) {
        errorsByIndex.set(rowIndex, 'Field name must be unique.');
      }
    }

    return errorsByIndex;
  }, [schemaFieldValidationErrors]);
  const errorIdPrefix = useMemo(
    () =>
      `${label.toLowerCase().replace(/[^a-z0-9]+/g, '-') || 'schema'}-field`,
    [label]
  );

  const handleAdd = () => {
    onChange([
      ...fields,
      { name: '', type: 'string', required: true, description: '' },
    ]);
  };

  const handleRemove = (index: number) => {
    const newFields = [...fields];
    newFields.splice(index, 1);
    onChange(newFields);
  };

  const handleChange = (
    index: number,
    field: keyof SchemaField,
    value: any
  ) => {
    const newFields = [...fields];
    newFields[index] = { ...newFields[index], [field]: value };
    onChange(newFields);
  };

  return (
    <div className="space-y-2">
      {!hideLabel && <Label className="text-sm font-medium">{label}</Label>}
      <div className="border rounded-lg overflow-x-auto">
        <table className="w-full min-w-[980px]">
          <thead>
            <tr className="border-b">
              <th className="text-left p-2 text-sm font-medium text-muted-foreground">
                Field Name
              </th>
              <th className="text-left p-2 text-sm font-medium text-muted-foreground">
                Type
              </th>
              <th className="w-20 text-center p-2 text-sm font-medium text-muted-foreground">
                Required
              </th>
              <th className="w-20 text-center p-2 text-sm font-medium text-muted-foreground">
                Nullable
              </th>
              <th className="text-left p-2 text-sm font-medium text-muted-foreground">
                Description
              </th>
              <th className="text-left p-2 text-sm font-medium text-muted-foreground">
                Default
              </th>
              <th className="text-left p-2 text-sm font-medium text-muted-foreground">
                Format
              </th>
              {showEnum && (
                <th className="text-left p-2 text-sm font-medium text-muted-foreground">
                  Enum
                </th>
              )}
              <th className="w-20 text-center p-2 text-sm font-medium text-muted-foreground">
                Advanced
              </th>
              {!readOnly && (
                <th className="w-16 text-center p-2 text-sm font-medium text-muted-foreground">
                  Actions
                </th>
              )}
            </tr>
          </thead>
          <tbody>
            {fields.map((field, index) => {
              const fieldNameError = fieldNameErrorsByIndex.get(index) ?? null;
              const fieldNameErrorId = fieldNameError
                ? `${errorIdPrefix}-${index}-name-error`
                : undefined;

              return (
                <tr key={index} className="border-b hover:bg-muted/30">
                  <td className="p-2 align-top">
                    <Input
                      value={field.name}
                      onChange={(e) =>
                        handleChange(index, 'name', e.target.value)
                      }
                      placeholder="fieldName"
                      disabled={readOnly}
                      aria-invalid={!!fieldNameError}
                      aria-describedby={fieldNameErrorId}
                      className={cn(
                        'font-mono text-sm border-0 p-1 h-auto focus-visible:ring-0 focus-visible:ring-offset-0',
                        fieldNameError &&
                          'bg-destructive/10 text-destructive focus-visible:ring-destructive'
                      )}
                    />
                    {fieldNameError && (
                      <p
                        id={fieldNameErrorId}
                        className="mt-1 text-xs text-destructive"
                      >
                        {fieldNameError}
                      </p>
                    )}
                  </td>
                  <td className="p-2 align-top">
                    <Select
                      value={field.type || 'string'}
                      onValueChange={(value) =>
                        handleChange(index, 'type', value)
                      }
                      disabled={readOnly}
                    >
                      <SelectTrigger className="h-7 border-0 focus:ring-0 focus:ring-offset-0">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {SUPPORTED_TYPES.map((type) => (
                          <SelectItem key={type.value} value={type.value}>
                            {type.label}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </td>
                  <td className="w-20 text-center p-2 align-top">
                    <Checkbox
                      checked={field.required}
                      onCheckedChange={(checked) =>
                        handleChange(index, 'required', !!checked)
                      }
                      disabled={readOnly}
                    />
                  </td>
                  <td className="w-20 text-center p-2 align-top">
                    <Checkbox
                      checked={!!field.nullable}
                      onCheckedChange={(checked) =>
                        handleChange(index, 'nullable', !!checked)
                      }
                      disabled={readOnly}
                    />
                  </td>
                  <td className="p-2 align-top">
                    <Input
                      value={field.description}
                      onChange={(e) =>
                        handleChange(index, 'description', e.target.value)
                      }
                      placeholder="Field description"
                      disabled={readOnly}
                      className="text-sm border-0 p-1 h-auto focus-visible:ring-0 focus-visible:ring-offset-0"
                    />
                  </td>
                  <td className="p-2 align-top">
                    <Input
                      value={formatFieldValue(field.defaultValue)}
                      onChange={(e) =>
                        handleChange(
                          index,
                          'defaultValue',
                          parseDefaultValue(e.target.value, field.type)
                        )
                      }
                      placeholder="Default"
                      disabled={readOnly}
                      className="font-mono text-sm border-0 p-1 h-auto focus-visible:ring-0 focus-visible:ring-offset-0"
                    />
                  </td>
                  <td className="p-2 align-top">
                    <Input
                      value={field.format || ''}
                      onChange={(e) =>
                        handleChange(
                          index,
                          'format',
                          e.target.value || undefined
                        )
                      }
                      placeholder="date, email..."
                      disabled={readOnly}
                      className="font-mono text-sm border-0 p-1 h-auto focus-visible:ring-0 focus-visible:ring-offset-0"
                    />
                  </td>
                  {showEnum && (
                    <td className="p-2 align-top">
                      <Input
                        value={formatEnumValue(field.enum)}
                        onChange={(e) => {
                          const newFields = [...fields];
                          newFields[index] = {
                            ...newFields[index],
                            enum: parseEnumValue(e.target.value),
                          };
                          onChange(newFields);
                        }}
                        placeholder="val1, val2, ..."
                        disabled={readOnly}
                        className="font-mono text-sm border-0 p-1 h-auto focus-visible:ring-0 focus-visible:ring-offset-0"
                      />
                    </td>
                  )}
                  <td className="w-20 text-center p-2 align-top">
                    <AdvancedSchemaFieldDialog
                      field={field}
                      readOnly={readOnly}
                      onApply={(updatedField) => {
                        const newFields = [...fields];
                        newFields[index] = updatedField;
                        onChange(newFields);
                      }}
                    />
                  </td>
                  {!readOnly && (
                    <td className="w-16 text-center p-2 align-top">
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        onClick={() => handleRemove(index)}
                        className="h-6 w-6 p-0"
                      >
                        <Trash2 className="h-3 w-3" />
                      </Button>
                    </td>
                  )}
                </tr>
              );
            })}
            {fields.length === 0 && (
              <tr>
                <td
                  colSpan={(readOnly ? 8 : 9) + (showEnum ? 1 : 0)}
                  className="p-4 text-center text-sm text-muted-foreground"
                >
                  {emptyMessage}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
      {!readOnly && (
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={handleAdd}
          className="w-full"
        >
          <Plus className="h-4 w-4 mr-2" />
          Add Field
        </Button>
      )}
    </div>
  );
}
