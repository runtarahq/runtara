import { useEffect, useMemo, useState } from 'react';
import { Button } from '@/shared/components/ui/button';
import { Checkbox } from '@/shared/components/ui/checkbox';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import { Plus, Trash2 } from 'lucide-react';
import { cn } from '@/lib/utils';
import { validateSchemaFieldsWithRust } from '@/features/workflows/utils/rust-workflow-validation';
import type { RustSchemaFieldsValidationError } from '@/features/workflows/utils/rust-workflow-validation';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';

const SUPPORTED_TYPES: { label: string; value: string }[] = [
  { label: 'String', value: 'string' },
  { label: 'Number', value: 'number' },
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
  enum?: string[];
  nullable?: boolean;
};

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
    value: string | boolean
  ) => {
    const newFields = [...fields];
    newFields[index] = { ...newFields[index], [field]: value };
    onChange(newFields);
  };

  return (
    <div className="space-y-2">
      {!hideLabel && <Label className="text-sm font-medium">{label}</Label>}
      <div className="border rounded-lg">
        <table className="w-full">
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
              <th className="text-left p-2 text-sm font-medium text-muted-foreground">
                Description
              </th>
              {showEnum && (
                <th className="text-left p-2 text-sm font-medium text-muted-foreground">
                  Enum
                </th>
              )}
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
                  {showEnum && (
                    <td className="p-2 align-top">
                      <Input
                        value={(field.enum || []).join(', ')}
                        onChange={(e) => {
                          const raw = e.target.value;
                          const enumValues = raw
                            ? raw
                                .split(',')
                                .map((v) => v.trim())
                                .filter(Boolean)
                            : [];
                          const newFields = [...fields];
                          newFields[index] = {
                            ...newFields[index],
                            enum:
                              enumValues.length > 0 ? enumValues : undefined,
                          };
                          onChange(newFields);
                        }}
                        placeholder="val1, val2, ..."
                        disabled={readOnly}
                        className="font-mono text-sm border-0 p-1 h-auto focus-visible:ring-0 focus-visible:ring-offset-0"
                      />
                    </td>
                  )}
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
                  colSpan={(readOnly ? 4 : 5) + (showEnum ? 1 : 0)}
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
