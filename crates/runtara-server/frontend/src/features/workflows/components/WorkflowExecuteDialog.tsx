import { useState, useEffect, useMemo } from 'react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/shared/components/ui/dialog';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import { parseSchema, SchemaField } from '@/features/workflows/utils/schema';
import { Checkbox } from '@/shared/components/ui/checkbox';
import { FileInput } from '@/shared/components/ui/file-input';
import { CompositeValueEditor } from '@/features/workflows/components/WorkflowEditor/NodeForm/InputMappingField/CompositeValueEditor';
import type {
  CompositeArrayValue,
  CompositeObjectValue,
  CompositeValue,
} from '@/features/workflows/stores/nodeFormStore';
import { X } from 'lucide-react';

type WorkflowExecuteDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  workflowName?: string;
  inputSchema?: any;
  onExecute: (inputData: Record<string, any>) => void;
  isSubmitting?: boolean;
  serverError?: string | null;
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

export function WorkflowExecuteDialog({
  open,
  onOpenChange,
  workflowName,
  inputSchema,
  onExecute,
  isSubmitting = false,
  serverError,
}: WorkflowExecuteDialogProps) {
  const fields = useMemo(() => parseSchema(inputSchema), [inputSchema]);

  const getInitialData = (schemaFields: SchemaField[]) => {
    const initial: Record<string, any> = {};
    schemaFields.forEach((field) => {
      if (field.defaultValue !== undefined) {
        initial[field.name] = field.defaultValue;
      } else if (field.type === 'boolean') {
        initial[field.name] = false;
      }
      // All other types without defaults start as undefined (not in the record)
    });
    return initial;
  };

  const getInitialTouched = (schemaFields: SchemaField[]) => {
    const touched = new Set<string>();
    schemaFields.forEach((field) => {
      if (field.defaultValue !== undefined) {
        touched.add(field.name);
      } else if (field.type === 'boolean') {
        touched.add(field.name);
      }
    });
    return touched;
  };

  const [inputData, setInputData] = useState<Record<string, any>>(() =>
    getInitialData(fields)
  );
  const [compositeInputData, setCompositeInputData] = useState<
    Record<string, CompositeObjectValue | CompositeArrayValue>
  >({});
  const [touchedFields, setTouchedFields] = useState<Set<string>>(() =>
    getInitialTouched(fields)
  );

  const [validationErrors, setValidationErrors] = useState<
    Record<string, string>
  >({});

  // Reset input data and validation errors when input schema changes
  useEffect(() => {
    const initialData = getInitialData(fields);
    const initialCompositeData = fields.reduce<
      Record<string, CompositeObjectValue | CompositeArrayValue>
    >((acc, field) => {
      if (field.type !== 'array' && field.type !== 'object') {
        return acc;
      }

      // Only initialize composite data for fields that have a value
      if (initialData[field.name] !== undefined) {
        acc[field.name] = convertPlainToComposite(
          parseComplexFieldValue(initialData[field.name], field.type)
        );
      }
      return acc;
    }, {});

    setInputData(initialData);
    setCompositeInputData(initialCompositeData);
    setTouchedFields(getInitialTouched(fields));
    setValidationErrors({});
  }, [fields]);

  // Reset input data and validation errors when dialog opens
  useEffect(() => {
    if (open) {
      const initialData = getInitialData(fields);
      const initialCompositeData = fields.reduce<
        Record<string, CompositeObjectValue | CompositeArrayValue>
      >((acc, field) => {
        if (field.type !== 'array' && field.type !== 'object') {
          return acc;
        }

        if (initialData[field.name] !== undefined) {
          acc[field.name] = convertPlainToComposite(
            parseComplexFieldValue(initialData[field.name], field.type)
          );
        }
        return acc;
      }, {});

      setInputData(initialData);
      setCompositeInputData(initialCompositeData);
      setTouchedFields(getInitialTouched(fields));
      setValidationErrors({});
    }
  }, [open, fields]);

  const handleFieldChange = (fieldName: string, value: any) => {
    setInputData((prev) => ({
      ...prev,
      [fieldName]: value,
    }));
    setTouchedFields((prev) => new Set(prev).add(fieldName));
    // Clear validation error for this field when user makes changes
    if (validationErrors[fieldName]) {
      setValidationErrors((prev) => {
        const { [fieldName]: _removed, ...rest } = prev;
        void _removed;
        return rest;
      });
    }
  };

  const handleFieldClear = (fieldName: string) => {
    setInputData((prev) => {
      const { [fieldName]: _removed, ...rest } = prev;
      void _removed;
      return rest;
    });
    setCompositeInputData((prev) => {
      const { [fieldName]: _removed, ...rest } = prev;
      void _removed;
      return rest;
    });
    setTouchedFields((prev) => {
      const next = new Set(prev);
      next.delete(fieldName);
      return next;
    });
    if (validationErrors[fieldName]) {
      setValidationErrors((prev) => {
        const { [fieldName]: _removed, ...rest } = prev;
        void _removed;
        return rest;
      });
    }
  };

  const getValidationErrors = (): Record<string, string> => {
    const errors: Record<string, string> = {};
    fields.forEach((field) => {
      if (field.required) {
        const value = inputData[field.name];
        if (
          !touchedFields.has(field.name) ||
          value === undefined ||
          value === null
        ) {
          errors[field.name] = `${field.name} is required`;
        }
      }
    });
    return errors;
  };

  const handleExecute = () => {
    const errors = getValidationErrors();
    setValidationErrors(errors);
    if (Object.keys(errors).length > 0) {
      return;
    }
    // Only include fields that are touched and defined
    const filteredData: Record<string, any> = {};
    for (const [key, value] of Object.entries(inputData)) {
      if (touchedFields.has(key) && value !== undefined) {
        filteredData[key] = value;
      }
    }
    onExecute(filteredData);
  };

  const renderField = (field: SchemaField) => {
    const value = inputData[field.name];
    const error = validationErrors[field.name];
    const isTouched = touchedFields.has(field.name);

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
                checked={value ?? false}
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
              value={value ?? ''}
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
              value={value ?? ''}
              onChange={(e) => handleFieldChange(field.name, e.target.value)}
              placeholder={field.description}
              className={error ? 'border-destructive' : ''}
            />
            {error && <p className="text-xs text-destructive">{error}</p>}
          </div>
        );

      case 'array':
      case 'object': {
        const isSet = isTouched && value !== undefined;

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
                    handleFieldChange(field.name, empty);
                    setCompositeInputData((prev) => ({
                      ...prev,
                      [field.name]: convertPlainToComposite(empty),
                    }));
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
          convertPlainToComposite(parseComplexFieldValue(value, field.type));

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
                typeof value === 'string'
                  ? value
                  : value
                    ? JSON.stringify(value)
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
              value={value ?? ''}
              onChange={(e) => handleFieldChange(field.name, e.target.value)}
              placeholder={field.description}
              className={error ? 'border-destructive' : ''}
            />
            {error && <p className="text-xs text-destructive">{error}</p>}
          </div>
        );
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl max-h-[80vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle>
            Execute Workflow{workflowName ? `: ${workflowName}` : ''}
          </DialogTitle>
          <DialogDescription>
            This workflow requires input data. Please provide the required
            fields below.
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4 py-4">
          {fields.length === 0 ? (
            <p className="text-sm text-muted-foreground">
              No input fields required.
            </p>
          ) : (
            fields.map((field) => (
              <div key={field.name}>{renderField(field)}</div>
            ))
          )}
          {serverError && (
            <div className="rounded-md bg-destructive/10 p-3 text-sm text-destructive">
              {serverError}
            </div>
          )}
        </div>

        <DialogFooter>
          <Button
            variant="outline"
            onClick={() => onOpenChange(false)}
            disabled={isSubmitting}
          >
            Cancel
          </Button>
          <Button onClick={handleExecute} disabled={isSubmitting}>
            {isSubmitting ? 'Executing...' : 'Execute'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
