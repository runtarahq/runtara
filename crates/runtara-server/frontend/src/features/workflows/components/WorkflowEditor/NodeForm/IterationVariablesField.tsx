import { Fragment, useMemo, useState } from 'react';
import { useFieldArray, useFormContext, useWatch } from 'react-hook-form';
import { ChevronRight, Plus, Trash2 } from 'lucide-react';
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
import {
  MappingValueInput,
  ValueMode,
} from './InputMappingField/MappingValueInput';
import { CompositeValueEditor } from './InputMappingField/CompositeValueEditor';
import { ModeToggleButton } from './InputMappingField/ModeToggleButton';
import { VariablePickerModal } from './InputMappingField/VariablePickerModal';
import type { VariableSuggestion } from './InputMappingValueField/VariableSuggestions';
import type {
  CompositeArrayValue,
  CompositeObjectValue,
} from '@/features/workflows/stores/nodeFormStore';

type IterationVariableField = {
  name?: string;
  value?: unknown;
  type?: string;
  valueType?: 'reference' | 'immediate' | 'composite' | 'template';
};

const VARIABLE_TYPES = [
  { label: 'String', value: 'string' },
  { label: 'Number', value: 'number' },
  { label: 'Boolean', value: 'boolean' },
  { label: 'Object', value: 'object' },
  { label: 'Array', value: 'array' },
  { label: 'File', value: 'file' },
];

type IterationVariablesFieldProps = {
  fieldArrayName: 'splitVariablesFields' | 'whileVariablesFields';
  description: string;
};

function parseObject(value: unknown): CompositeObjectValue {
  if (typeof value === 'object' && value !== null && !Array.isArray(value)) {
    return value as CompositeObjectValue;
  }
  if (typeof value === 'string' && value.trim()) {
    try {
      const parsed = JSON.parse(value);
      if (
        typeof parsed === 'object' &&
        parsed !== null &&
        !Array.isArray(parsed)
      ) {
        return parsed as CompositeObjectValue;
      }
    } catch {
      // Invalid JSON starts from an empty structured value.
    }
  }
  return {};
}

function parseArray(value: unknown): CompositeArrayValue {
  if (Array.isArray(value)) return value as CompositeArrayValue;
  if (typeof value === 'string' && value.trim()) {
    try {
      const parsed = JSON.parse(value);
      if (Array.isArray(parsed)) return parsed as CompositeArrayValue;
    } catch {
      // Invalid JSON starts from an empty structured value.
    }
  }
  return [];
}

export function IterationVariablesField({
  fieldArrayName,
  description,
}: IterationVariablesFieldProps) {
  const form = useFormContext();
  const [editingIndex, setEditingIndex] = useState<number | null>(null);
  const [pickerIndex, setPickerIndex] = useState<number | null>(null);
  const { fields, append, remove } = useFieldArray({
    name: fieldArrayName,
    control: form.control,
  });
  const watchedValues = useWatch({
    name: fieldArrayName,
    control: form.control,
  });
  const values = useMemo(
    () => (watchedValues || []) as IterationVariableField[],
    [watchedValues]
  );

  return (
    <div className="space-y-2">
      <Label className="text-sm font-medium">Variables</Label>
      <p className="text-xs text-muted-foreground">{description}</p>
      <div className="border rounded-lg">
        <table className="w-full">
          <thead>
            <tr className="border-b">
              <th className="text-left p-2 text-sm font-medium text-muted-foreground">
                Name
              </th>
              <th className="text-left p-2 text-sm font-medium text-muted-foreground">
                Value
              </th>
              <th className="text-left p-2 text-sm font-medium text-muted-foreground w-28">
                Type
              </th>
              <th className="w-16 text-center p-2 text-sm font-medium text-muted-foreground">
                Actions
              </th>
            </tr>
          </thead>
          <tbody>
            {fields.map((field, index) => {
              const variable = values[index] || {};
              const variableType = variable.type || 'string';
              const isArray = variableType === 'array';
              const isStructured = isArray || variableType === 'object';
              const isReference = variable.valueType === 'reference';
              const scalarValue =
                typeof variable.value === 'string'
                  ? variable.value
                  : variable.value === undefined || variable.value === null
                    ? ''
                    : JSON.stringify(variable.value);
              const scalarValueType: ValueMode =
                variable.valueType === 'reference'
                  ? 'reference'
                  : variable.valueType === 'template'
                    ? 'template'
                    : 'immediate';
              const path = `${fieldArrayName}.${index}`;
              const structuredValue = isArray
                ? parseArray(variable.value)
                : parseObject(variable.value);
              const itemCount = isArray
                ? (structuredValue as CompositeArrayValue).length
                : Object.keys(structuredValue as CompositeObjectValue).length;

              return (
                <Fragment key={field.id}>
                  <tr className="border-b hover:bg-muted/30">
                    <td className="p-2">
                      <Input
                        {...form.register(`${path}.name`)}
                        placeholder="variableName"
                        className="font-mono text-sm border-0 p-1 h-auto focus-visible:ring-0"
                      />
                      {form.getFieldState(`${path}.name`, form.formState).error
                        ?.message && (
                        <p className="text-xs text-destructive mt-1">
                          {
                            form.getFieldState(`${path}.name`, form.formState)
                              .error?.message
                          }
                        </p>
                      )}
                    </td>
                    <td className="p-2">
                      {isStructured ? (
                        <div className="flex items-start gap-2">
                          {isReference ? (
                            <MappingValueInput
                              value={
                                typeof variable.value === 'string'
                                  ? variable.value
                                  : ''
                              }
                              onChange={(value) =>
                                form.setValue(`${path}.value`, value, {
                                  shouldDirty: true,
                                })
                              }
                              valueType="reference"
                              onValueTypeChange={() => {
                                form.setValue(
                                  `${path}.valueType`,
                                  'composite',
                                  {
                                    shouldDirty: true,
                                  }
                                );
                                form.setValue(
                                  `${path}.value`,
                                  isArray ? [] : {},
                                  {
                                    shouldDirty: true,
                                  }
                                );
                                setEditingIndex(index);
                              }}
                              fieldType={variableType}
                              placeholder="Select reference..."
                              hideReferenceToggle
                            />
                          ) : (
                            <button
                              type="button"
                              onClick={() => {
                                form.setValue(
                                  `${path}.valueType`,
                                  'composite',
                                  {
                                    shouldDirty: true,
                                  }
                                );
                                form.setValue(
                                  `${path}.value`,
                                  structuredValue,
                                  {
                                    shouldDirty: true,
                                  }
                                );
                                setEditingIndex((current) =>
                                  current === index ? null : index
                                );
                              }}
                              className="w-full flex items-center justify-between gap-2 px-3 py-2 text-sm border rounded-md bg-muted/30 hover:bg-muted/50 transition-colors text-left"
                            >
                              <span className="text-muted-foreground truncate">
                                Composite: {itemCount}{' '}
                                {isArray
                                  ? `item${itemCount === 1 ? '' : 's'}`
                                  : `field${itemCount === 1 ? '' : 's'}`}
                              </span>
                              <ChevronRight className="h-4 w-4 text-muted-foreground shrink-0" />
                            </button>
                          )}
                          <ModeToggleButton
                            mode={isReference ? 'reference' : 'immediate'}
                            onClick={() => {
                              if (isReference) {
                                form.setValue(
                                  `${path}.valueType`,
                                  'composite',
                                  {
                                    shouldDirty: true,
                                  }
                                );
                                form.setValue(
                                  `${path}.value`,
                                  isArray ? [] : {},
                                  {
                                    shouldDirty: true,
                                  }
                                );
                                setEditingIndex(index);
                              } else {
                                if (editingIndex === index)
                                  setEditingIndex(null);
                                setPickerIndex(index);
                              }
                            }}
                          />
                        </div>
                      ) : (
                        <MappingValueInput
                          value={scalarValue}
                          onChange={(value) =>
                            form.setValue(`${path}.value`, value, {
                              shouldDirty: true,
                            })
                          }
                          valueType={scalarValueType}
                          onValueTypeChange={(valueType) =>
                            form.setValue(`${path}.valueType`, valueType, {
                              shouldDirty: true,
                            })
                          }
                          fieldType={variableType}
                          placeholder="Select reference or enter value"
                        />
                      )}
                      {form.getFieldState(`${path}.value`, form.formState).error
                        ?.message && (
                        <p className="text-xs text-destructive mt-1">
                          {
                            form.getFieldState(`${path}.value`, form.formState)
                              .error?.message
                          }
                        </p>
                      )}
                      {form.getFieldState(`${path}.valueType`, form.formState)
                        .error?.message && (
                        <p className="text-xs text-destructive mt-1">
                          Unsupported value mode:{' '}
                          {
                            form.getFieldState(
                              `${path}.valueType`,
                              form.formState
                            ).error?.message
                          }
                        </p>
                      )}
                    </td>
                    <td className="p-2">
                      <Select
                        value={variableType}
                        onValueChange={(value) => {
                          form.setValue(`${path}.type`, value, {
                            shouldDirty: true,
                          });
                          if (value === 'object' || value === 'array') {
                            form.setValue(`${path}.valueType`, 'composite', {
                              shouldDirty: true,
                            });
                            form.setValue(
                              `${path}.value`,
                              value === 'array'
                                ? parseArray(variable.value)
                                : parseObject(variable.value),
                              { shouldDirty: true }
                            );
                            setPickerIndex(null);
                            setEditingIndex(index);
                          } else {
                            if (editingIndex === index) setEditingIndex(null);
                            if (pickerIndex === index) setPickerIndex(null);
                            form.setValue(`${path}.valueType`, 'immediate', {
                              shouldDirty: true,
                            });
                          }
                        }}
                      >
                        <SelectTrigger className="h-7 border-0 focus:ring-0">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          {VARIABLE_TYPES.map((type) => (
                            <SelectItem key={type.value} value={type.value}>
                              {type.label}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </td>
                    <td className="w-16 text-center p-2">
                      <Button
                        type="button"
                        variant="ghost"
                        size="sm"
                        onClick={() => {
                          if (editingIndex === index) setEditingIndex(null);
                          if (pickerIndex === index) setPickerIndex(null);
                          remove(index);
                        }}
                        className="h-6 w-6 p-0"
                      >
                        <Trash2 className="h-3 w-3" />
                      </Button>
                    </td>
                  </tr>
                  {isStructured && !isReference && editingIndex === index && (
                    <tr className="hover:bg-transparent">
                      <td colSpan={4} className="p-0 border-t-0">
                        <div className="border-t border-primary/20 bg-muted/20">
                          <CompositeValueEditor
                            value={structuredValue}
                            onChange={(value) => {
                              form.setValue(`${path}.valueType`, 'composite', {
                                shouldDirty: true,
                              });
                              form.setValue(`${path}.value`, value, {
                                shouldDirty: true,
                              });
                            }}
                            onClose={() => setEditingIndex(null)}
                            showModeSwitcher={false}
                          />
                        </div>
                      </td>
                    </tr>
                  )}
                </Fragment>
              );
            })}
            {fields.length === 0 && (
              <tr>
                <td
                  colSpan={4}
                  className="p-4 text-center text-sm text-muted-foreground"
                >
                  No variables defined.
                </td>
              </tr>
            )}
          </tbody>
        </table>
        <VariablePickerModal
          open={pickerIndex !== null}
          onOpenChange={(open) => {
            if (!open) setPickerIndex(null);
          }}
          onSelect={(selected: VariableSuggestion) => {
            if (pickerIndex === null) return;
            form.setValue(
              `${fieldArrayName}.${pickerIndex}.valueType`,
              'reference',
              {
                shouldDirty: true,
              }
            );
            form.setValue(
              `${fieldArrayName}.${pickerIndex}.value`,
              selected.value,
              { shouldDirty: true }
            );
            if (editingIndex === pickerIndex) setEditingIndex(null);
            setPickerIndex(null);
          }}
        />
      </div>
      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() =>
          append({
            name: '',
            value: '',
            valueType: 'immediate',
            type: 'string',
          })
        }
        className="w-full"
      >
        <Plus className="h-4 w-4 mr-2" />
        Add Variable
      </Button>
    </div>
  );
}
