import { useEffect, useMemo, useState } from 'react';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import { Checkbox } from '@/shared/components/ui/checkbox';
import { FileInput } from '@/shared/components/ui/file-input';
import { parseSchema, SchemaField } from '@/features/workflows/utils/schema';
import { CompositeValueEditor } from '@/features/workflows/components/WorkflowEditor/NodeForm/InputMappingField/CompositeValueEditor';
import type {
  CompositeArrayValue,
  CompositeObjectValue,
  CompositeValue,
} from '@/features/workflows/stores/nodeFormStore';
import { X } from 'lucide-react';

// Schema-driven input form extracted from WorkflowExecuteDialog so the same
// renderer backs both the workflow Run dialog and the CRON trigger's static
// inputs editor. Controlled for data: `value` is the data object, key
// presence marks a field as "set" (touched); absent keys render untouched.
// Composite (array/object) editing scratch state stays internal so in-flight
// CompositeValueEditor edits are not lost to plain<->composite round-trips.

export type SchemaInputFormChangeAction = 'set' | 'clear';

export type SchemaInputFormProps = {
  /** Raw input schema (object or JSON string); parsed with parseSchema. */
  inputSchema?: any;
  /** Current data object. Key presence = field is set. */
  value: Record<string, any>;
  /**
   * Called with the next data object whenever a field is edited ('set') or
   * cleared ('clear'). Keys in `value` that no schema field covers are
   * passed through untouched.
   */
  onChange: (
    next: Record<string, any>,
    changedField: string,
    action: SchemaInputFormChangeAction
  ) => void;
  /** Per-field validation errors rendered under the matching field. */
  errors?: Record<string, string>;
};

const convertPlainToComposite = (
  value: unknown
): CompositeObjectValue | CompositeArrayValue => {
  if (Array.isArray(value)) {
    return value.map((item) => {
      if (Array.isArray(item) || (typeof item === 'object' && item !== null)) {
        return {
          valueType: 'composite' as const,
          value: convertPlainToComposite(item),
        };
      }

      return {
        valueType: 'immediate' as const,
        value: item as string | number | boolean | null,
      };
    });
  }

  if (typeof value === 'object' && value !== null) {
    return Object.entries(value).reduce<CompositeObjectValue>(
      (acc, [key, val]) => {
        if (Array.isArray(val) || (typeof val === 'object' && val !== null)) {
          acc[key] = {
            valueType: 'composite' as const,
            value: convertPlainToComposite(val),
          };
        } else {
          acc[key] = {
            valueType: 'immediate' as const,
            value: val as string | number | boolean | null,
          };
        }

        return acc;
      },
      {}
    );
  }

  return {};
};

const coerceImmediateValue = (
  value: string | number | boolean | null,
  typeHint?: string
): string | number | boolean | null => {
  if (!typeHint || value === null) return value;
  if (typeHint === 'integer' || typeHint === 'number') {
    const numValue = Number(value);
    if (!isNaN(numValue)) {
      return typeHint === 'integer' ? Math.trunc(numValue) : numValue;
    }
  }
  if (typeHint === 'boolean' && typeof value === 'string') {
    const lower = value.toLowerCase();
    if (lower === 'true' || lower === '1') return true;
    if (lower === 'false' || lower === '0') return false;
  }
  return value;
};

const convertCompositeToPlain = (value: CompositeValue): unknown => {
  if (value.valueType === 'composite') {
    if (Array.isArray(value.value)) {
      return value.value.map((item) => convertCompositeToPlain(item));
    }

    return Object.entries(value.value).reduce<Record<string, unknown>>(
      (acc, [key, nestedValue]) => {
        acc[key] = convertCompositeToPlain(nestedValue);
        return acc;
      },
      {}
    );
  }

  if (value.valueType === 'immediate') {
    return coerceImmediateValue(value.value, value.typeHint);
  }

  return value.value;
};

const convertCompositeRootToPlain = (
  value: CompositeObjectValue | CompositeArrayValue
): Record<string, unknown> | unknown[] => {
  if (Array.isArray(value)) {
    return value.map((item) => convertCompositeToPlain(item));
  }

  return Object.entries(value).reduce<Record<string, unknown>>(
    (acc, [key, nestedValue]) => {
      acc[key] = convertCompositeToPlain(nestedValue);
      return acc;
    },
    {}
  );
};

const parseComplexFieldValue = (
  rawValue: unknown,
  fieldType: 'array' | 'object'
): Record<string, unknown> | unknown[] => {
  if (fieldType === 'array') {
    if (Array.isArray(rawValue)) {
      return rawValue;
    }

    if (typeof rawValue === 'string') {
      try {
        const parsed = JSON.parse(rawValue);
        return Array.isArray(parsed) ? parsed : [];
      } catch {
        return [];
      }
    }

    return [];
  }

  if (
    typeof rawValue === 'object' &&
    rawValue !== null &&
    !Array.isArray(rawValue)
  ) {
    return rawValue as Record<string, unknown>;
  }

  if (typeof rawValue === 'string') {
    try {
      const parsed = JSON.parse(rawValue);
      return typeof parsed === 'object' &&
        parsed !== null &&
        !Array.isArray(parsed)
        ? parsed
        : {};
    } catch {
      return {};
    }
  }

  return {};
};

const EMPTY_ERRORS: Record<string, string> = {};

export function SchemaInputForm({
  inputSchema,
  value,
  onChange,
  errors = EMPTY_ERRORS,
}: SchemaInputFormProps) {
  const fields = useMemo(() => parseSchema(inputSchema), [inputSchema]);

  // Editing scratch state for array/object fields; reset when the schema
  // changes (mirrors the pre-extraction reset-on-schema-change behavior).
  const [compositeInputData, setCompositeInputData] = useState<
    Record<string, CompositeObjectValue | CompositeArrayValue>
  >({});

  useEffect(() => {
    setCompositeInputData({});
  }, [fields]);

  const handleFieldChange = (fieldName: string, fieldValue: any) => {
    onChange({ ...value, [fieldName]: fieldValue }, fieldName, 'set');
  };

  const handleFieldClear = (fieldName: string) => {
    const { [fieldName]: _removed, ...rest } = value;
    void _removed;
    setCompositeInputData((prev) => {
      const { [fieldName]: _removedComposite, ...restComposite } = prev;
      void _removedComposite;
      return restComposite;
    });
    onChange(rest, fieldName, 'clear');
  };

  const renderField = (field: SchemaField) => {
    const fieldValue = value[field.name];
    const error = errors[field.name];
    const isTouched = field.name in value;

    const clearButton = !field.required && isTouched && (
      <Button
        variant="ghost"
        size="sm"
        onClick={() => handleFieldClear(field.name)}
        className="h-5 w-5 p-0 hover:bg-muted ml-1"
        title="Clear field"
      >
        <X className="h-3 w-3" />
      </Button>
    );

    switch (field.type) {
      case 'boolean':
        return (
          <div className="space-y-2">
            <div className="flex items-center space-x-2">
              <Checkbox
                id={field.name}
                checked={fieldValue ?? false}
                onCheckedChange={(checked) =>
                  handleFieldChange(field.name, checked)
                }
              />
              <Label htmlFor={field.name} className="text-sm font-normal">
                {field.description || field.name}
              </Label>
              {clearButton}
            </div>
            {error && <p className="text-xs text-destructive">{error}</p>}
          </div>
        );

      case 'number':
      case 'integer':
        return (
          <div className="space-y-2">
            <div className="flex items-center">
              <Label htmlFor={field.name}>
                {field.name}
                {field.required && <span className="text-destructive"> *</span>}
              </Label>
              {clearButton}
            </div>
            <Input
              id={field.name}
              type="number"
              step={field.type === 'integer' ? '1' : undefined}
              value={fieldValue ?? ''}
              onChange={(e) => {
                const raw = e.target.value;
                if (raw === '') {
                  handleFieldChange(field.name, undefined);
                } else {
                  const num =
                    field.type === 'integer'
                      ? parseInt(raw, 10)
                      : parseFloat(raw);
                  handleFieldChange(field.name, isNaN(num) ? undefined : num);
                }
              }}
              placeholder={field.description}
              className={error ? 'border-destructive' : ''}
            />
            {error && <p className="text-xs text-destructive">{error}</p>}
          </div>
        );

      case 'string':
        return (
          <div className="space-y-2">
            <div className="flex items-center">
              <Label htmlFor={field.name}>
                {field.name}
                {field.required && <span className="text-destructive"> *</span>}
              </Label>
              {clearButton}
            </div>
            <Input
              id={field.name}
              type="text"
              value={fieldValue ?? ''}
              onChange={(e) => handleFieldChange(field.name, e.target.value)}
              placeholder={field.description}
              className={error ? 'border-destructive' : ''}
            />
            {error && <p className="text-xs text-destructive">{error}</p>}
          </div>
        );

      case 'array':
      case 'object': {
        const isSet = isTouched && fieldValue !== undefined;

        if (!isSet) {
          return (
            <div className="space-y-2">
              <Label>
                {field.name}
                {field.required && <span className="text-destructive"> *</span>}
              </Label>
              <div>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => {
                    const empty = field.type === 'array' ? [] : {};
                    setCompositeInputData((prev) => ({
                      ...prev,
                      [field.name]: convertPlainToComposite(empty),
                    }));
                    handleFieldChange(field.name, empty);
                  }}
                >
                  Set value
                </Button>
              </div>
              {error && <p className="text-xs text-destructive">{error}</p>}
            </div>
          );
        }

        const currentCompositeValue =
          compositeInputData[field.name] ||
          convertPlainToComposite(
            parseComplexFieldValue(fieldValue, field.type)
          );

        return (
          <div className="space-y-2">
            <div className="flex items-center">
              <Label>
                {field.name}
                {field.required && <span className="text-destructive"> *</span>}
              </Label>
              {clearButton}
            </div>
            <div className="rounded-md border border-input overflow-hidden">
              <CompositeValueEditor
                value={currentCompositeValue}
                onChange={(newCompositeValue) => {
                  setCompositeInputData((prev) => ({
                    ...prev,
                    [field.name]: newCompositeValue,
                  }));
                  handleFieldChange(
                    field.name,
                    convertCompositeRootToPlain(newCompositeValue)
                  );
                }}
                showModeSwitcher={false}
                showCloseButton={false}
                title={
                  field.type === 'array'
                    ? 'Composite Array'
                    : 'Composite Object'
                }
              />
            </div>
            {error && <p className="text-xs text-destructive">{error}</p>}
          </div>
        );
      }

      case 'file':
        return (
          <div className="space-y-2">
            <Label htmlFor={field.name}>
              {field.name}
              {field.required && <span className="text-destructive"> *</span>}
            </Label>
            <FileInput
              value={
                typeof fieldValue === 'string'
                  ? fieldValue
                  : fieldValue
                    ? JSON.stringify(fieldValue)
                    : ''
              }
              onChange={(val) => {
                try {
                  handleFieldChange(field.name, val ? JSON.parse(val) : null);
                } catch {
                  handleFieldChange(field.name, val);
                }
              }}
              placeholder={field.description || 'Upload a file'}
            />
            {error && <p className="text-xs text-destructive">{error}</p>}
          </div>
        );

      default:
        return (
          <div className="space-y-2">
            <div className="flex items-center">
              <Label htmlFor={field.name}>
                {field.name}
                {field.required && <span className="text-destructive"> *</span>}
              </Label>
              {clearButton}
            </div>
            <Input
              id={field.name}
              type="text"
              value={fieldValue ?? ''}
              onChange={(e) => handleFieldChange(field.name, e.target.value)}
              placeholder={field.description}
              className={error ? 'border-destructive' : ''}
            />
            {error && <p className="text-xs text-destructive">{error}</p>}
          </div>
        );
    }
  };

  if (fields.length === 0) {
    return null;
  }

  return (
    <div className="space-y-4">
      {fields.map((field) => (
        <div key={field.name}>{renderField(field)}</div>
      ))}
    </div>
  );
}
