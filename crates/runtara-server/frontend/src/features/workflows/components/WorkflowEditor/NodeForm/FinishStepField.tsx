import { useContext, useEffect, useRef, useMemo } from 'react';
import { useFieldArray, useFormContext, useWatch } from 'react-hook-form';
import { SchemaPreview } from '@/features/workflows/components/SchemaPreview';
import { inferSchemaFromMapping } from '@/features/workflows/utils/schema';
import { Button } from '@/shared/components/ui/button';
import {
  FormControl,
  FormField,
  FormItem,
  FormMessage,
} from '@/shared/components/ui/form';
import { Input } from '@/shared/components/ui/input';
import { Plus, Trash2, ChevronDown } from 'lucide-react';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/shared/components/ui/dropdown-menu';
import { NodeFormContext } from './NodeFormContext';
import {
  MappingValueInput,
  ValueMode,
} from './InputMappingField/MappingValueInput';
import { ObjectMappingEditor } from './InputMappingField/ObjectMappingEditor';
import type {
  CompositeObjectValue,
  CompositeArrayValue,
} from '@/features/workflows/stores/nodeFormStore';

type FinishStepFieldProps = {
  name: string;
};

/** Available types for output fields */
const OUTPUT_FIELD_TYPES = [
  { value: 'string', short: 's', full: 'String' },
  { value: 'integer', short: 'n', full: 'Integer' },
  { value: 'number', short: 'n', full: 'Number' },
  { value: 'boolean', short: 'b', full: 'Boolean' },
  { value: 'object', short: '{}', full: 'Object' },
  { value: 'array', short: '[]', full: 'Array' },
];

/**
 * Maps schema field type to a display abbreviation
 */
function getTypeAbbreviation(type: string | undefined): {
  short: string;
  full: string;
} {
  if (!type) return { short: 's', full: 'String' };

  const lowerType = type.toLowerCase();

  if (lowerType === 'string') return { short: 's', full: 'String' };
  if (lowerType === 'number' || lowerType === 'integer')
    return { short: 'n', full: 'Number' };
  if (lowerType === 'boolean') return { short: 'b', full: 'Boolean' };
  if (lowerType === 'object') return { short: '{}', full: 'Object' };
  if (lowerType === 'array') return { short: '[]', full: 'Array' };
  if (lowerType === 'file') return { short: 'f', full: 'File' };

  return { short: type[0].toLowerCase(), full: type };
}

/** Get effective type display based on valueType mode */
function getEffectiveTypeInfo(
  schemaType: string | undefined,
  valueType: string | undefined,
  typeHint: string | undefined
): { short: string; full: string } {
  // Template mode always produces strings
  if (valueType === 'template') return { short: 's', full: 'String' };
  // Composite mode always produces objects
  if (valueType === 'composite') return { short: '{}', full: 'Object' };
  // Schema type takes priority
  if (schemaType) return getTypeAbbreviation(schemaType);
  // Fall back to typeHint or default to string
  return getTypeAbbreviation(typeHint || 'string');
}

export function FinishStepField({ name }: FinishStepFieldProps) {
  const form = useFormContext();
  const { outputSchemaFields = [], nodeId } = useContext(NodeFormContext);
  const stepType = useWatch({ name: 'stepType', control: form.control });
  const isEdit = !!nodeId;

  const { fields, append, remove } = useFieldArray({
    name,
    control: form.control,
  });

  const watchFieldArray = useWatch({ name, defaultValue: [] });
  const fieldArray = useMemo(
    () => (Array.isArray(watchFieldArray) ? watchFieldArray : []),
    [watchFieldArray]
  );

  // Track if we've auto-populated to prevent duplicates
  const autoPopulatedRef = useRef<boolean>(false);

  // Build schema field info map for quick lookup - must be before early return
  const schemaFieldMap = useMemo(() => {
    const map = new Map<
      string,
      { type: string; required: boolean; description: string }
    >();
    outputSchemaFields.forEach((field) => {
      map.set(field.name, {
        type: field.type,
        required: field.required,
        description: field.description,
      });
    });
    return map;
  }, [outputSchemaFields]);

  // Get optional fields that can still be added - must be before early return
  const availableOptionalFields = useMemo(() => {
    const existingFieldNames = new Set(
      fieldArray.map((f: any) => f.type).filter(Boolean)
    );
    return outputSchemaFields
      .filter((field) => !field.required)
      .filter((field) => !existingFieldNames.has(field.name));
  }, [outputSchemaFields, fieldArray]);

  // Get required fields that are missing - must be before early return
  const missingRequiredFields = useMemo(() => {
    const existingFieldNames = new Set(
      fieldArray.map((f: any) => f.type).filter(Boolean)
    );
    return outputSchemaFields
      .filter((field) => field.required)
      .filter((field) => !existingFieldNames.has(field.name));
  }, [outputSchemaFields, fieldArray]);

  // Auto-populate required fields when creating a new Finish step
  useEffect(() => {
    if (stepType !== 'Finish') return;
    if (isEdit) return; // Don't auto-populate in edit mode
    if (autoPopulatedRef.current) return;
    if (outputSchemaFields.length === 0) return;

    // Get required fields that aren't already present
    const existingFieldNames = new Set(
      fieldArray.map((f: any) => f.type).filter(Boolean)
    );

    const requiredFields = outputSchemaFields
      .filter((field) => field.required)
      .filter((field) => !existingFieldNames.has(field.name))
      .map((field) => ({
        type: field.name,
        value: '',
        typeHint: 'string' as const,
        valueType: 'immediate' as const,
      }));

    if (requiredFields.length > 0) {
      append(requiredFields);
      autoPopulatedRef.current = true;
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stepType, outputSchemaFields, isEdit]);

  // Early return after all hooks are called
  if (stepType !== 'Finish') {
    return null;
  }

  const schemaPreview = inferSchemaFromMapping(fields as any);

  const hasSchemaFields = outputSchemaFields.length > 0;

  return (
    <div className="space-y-4">
      <div className="flex items-start justify-between gap-2">
        <div>
          <p className="text-sm font-medium">Output configuration</p>
          <p className="text-xs text-muted-foreground">
            {hasSchemaFields
              ? 'Map workflow variables to output fields defined in the workflow schema.'
              : 'Map workflow variables into the final output payload.'}
          </p>
        </div>
      </div>

      {/* Warning for missing required fields */}
      {missingRequiredFields.length > 0 && (
        <div className="rounded-md border border-yellow-500/50 bg-yellow-500/10 p-3 text-sm">
          <p className="font-medium text-yellow-600 dark:text-yellow-400">
            Missing required fields:
          </p>
          <p className="text-muted-foreground">
            {missingRequiredFields.map((f) => f.name).join(', ')}
          </p>
          <Button
            type="button"
            size="sm"
            variant="outline"
            className="mt-2"
            onClick={() => {
              const newFields = missingRequiredFields.map((field) => ({
                type: field.name,
                value: '',
                typeHint: 'string' as const,
                valueType: 'immediate' as const,
              }));
              append(newFields);
            }}
          >
            Add missing required fields
          </Button>
        </div>
      )}

      {/* Output fields table */}
      <div className="border rounded-lg overflow-hidden">
        <table className="w-full table-fixed">
          <colgroup>
            <col className="w-10" />
            <col className="w-[30%]" />
            <col />
            <col className="w-10" />
          </colgroup>
          <thead>
            <tr className="border-b bg-muted/30">
              <th className="text-center p-2 text-xs font-medium text-muted-foreground">
                Type
              </th>
              <th className="text-left p-2 text-xs font-medium text-muted-foreground">
                Output Name
              </th>
              <th className="text-left p-2 text-xs font-medium text-muted-foreground">
                Source
              </th>
              <th className="text-center p-2 text-xs font-medium text-muted-foreground"></th>
            </tr>
          </thead>
          <tbody>
            {fields.map((field, index) => {
              const paramName = fieldArray[index]?.type || '';
              const fieldInfo = schemaFieldMap.get(paramName);
              const currentValueType = fieldArray[index]?.valueType;
              const currentTypeHint = fieldArray[index]?.typeHint;
              const typeInfo = getEffectiveTypeInfo(
                fieldInfo?.type,
                currentValueType,
                currentTypeHint
              );
              const isRequired = fieldInfo?.required ?? false;
              const isSchemaField = schemaFieldMap.has(paramName);
              const isCompositeMode = currentValueType === 'composite';
              // For non-schema fields, type is editable (unless overridden by mode)
              const isTypeEditable =
                !isSchemaField &&
                currentValueType !== 'template' &&
                currentValueType !== 'composite';

              return (
                <tr key={field.id} className="border-b hover:bg-muted/30">
                  <td className="text-center p-2">
                    {isTypeEditable ? (
                      <FormField
                        control={form.control}
                        name={`${name}.${index}.typeHint`}
                        render={({ field: typeHintField }) => (
                          <DropdownMenu>
                            <DropdownMenuTrigger asChild>
                              <button
                                type="button"
                                className="font-bold text-muted-foreground hover:text-foreground transition-colors cursor-pointer flex items-center gap-0.5 mx-auto"
                                title={`${typeInfo.full} — click to change`}
                              >
                                <span>{typeInfo.short}</span>
                                <ChevronDown className="h-2.5 w-2.5" />
                              </button>
                            </DropdownMenuTrigger>
                            <DropdownMenuContent align="start" className="w-32">
                              {OUTPUT_FIELD_TYPES.map((t) => (
                                <DropdownMenuItem
                                  key={t.value}
                                  onClick={() =>
                                    typeHintField.onChange(t.value)
                                  }
                                  className="text-xs"
                                >
                                  <span className="font-bold w-5">
                                    {t.short}
                                  </span>
                                  <span>{t.full}</span>
                                </DropdownMenuItem>
                              ))}
                            </DropdownMenuContent>
                          </DropdownMenu>
                        )}
                      />
                    ) : (
                      <span
                        className={`font-bold cursor-help ${
                          isSchemaField
                            ? 'text-green-600/70 dark:text-green-500/50'
                            : 'text-muted-foreground'
                        }`}
                        title={
                          isSchemaField
                            ? `${typeInfo.full}${isRequired ? ' (Required)' : ''}${fieldInfo?.description ? ` - ${fieldInfo.description}` : ''}`
                            : typeInfo.full
                        }
                      >
                        {typeInfo.short}
                      </span>
                    )}
                  </td>
                  <td className="p-2">
                    <FormField
                      control={form.control}
                      name={`${name}.${index}.type`}
                      render={({ field }) => (
                        <FormItem className="space-y-0">
                          <FormControl>
                            <div className="flex items-center gap-1">
                              <Input
                                {...field}
                                className={`font-mono text-sm border-0 p-1 h-auto focus-visible:ring-0 ${
                                  isRequired
                                    ? 'font-semibold'
                                    : 'text-muted-foreground'
                                }`}
                                placeholder="Output name"
                                readOnly={isSchemaField}
                              />
                              {isRequired && (
                                <span className="text-yellow-500 text-xs">
                                  *
                                </span>
                              )}
                            </div>
                          </FormControl>
                          <FormMessage />
                        </FormItem>
                      )}
                    />
                  </td>
                  <td className="p-2">
                    <FormField
                      control={form.control}
                      name={`${name}.${index}.value`}
                      render={({ field: valueField }) => (
                        <FormField
                          control={form.control}
                          name={`${name}.${index}.valueType`}
                          render={({ field: valueTypeField }) => (
                            <FormItem className="space-y-0">
                              <FormControl>
                                <div>
                                  <MappingValueInput
                                    value={
                                      isCompositeMode
                                        ? ''
                                        : valueField.value || ''
                                    }
                                    onChange={valueField.onChange}
                                    valueType={
                                      (valueTypeField.value as ValueMode) ||
                                      'immediate'
                                    }
                                    onValueTypeChange={valueTypeField.onChange}
                                    fieldType={fieldInfo?.type || 'string'}
                                    placeholder="Enter value or select reference..."
                                  />
                                  {isCompositeMode && (
                                    <div className="mt-2 border-t border-primary/20 bg-muted/20 rounded-b-md">
                                      <ObjectMappingEditor
                                        value={
                                          typeof valueField.value ===
                                            'object' &&
                                          valueField.value !== null
                                            ? (valueField.value as
                                                | CompositeObjectValue
                                                | CompositeArrayValue)
                                            : {}
                                        }
                                        valueType="composite"
                                        onChange={(val) =>
                                          valueField.onChange(val)
                                        }
                                        onValueTypeChange={(type) =>
                                          valueTypeField.onChange(type)
                                        }
                                        onClose={() =>
                                          valueTypeField.onChange('immediate')
                                        }
                                      />
                                    </div>
                                  )}
                                </div>
                              </FormControl>
                              <FormMessage />
                            </FormItem>
                          )}
                        />
                      )}
                    />
                  </td>
                  <td className="text-center p-2 pl-3">
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className="h-7 w-7"
                      onClick={() => remove(index)}
                      disabled={isRequired}
                    >
                      <Trash2 className="h-3 w-3" />
                    </Button>
                  </td>
                </tr>
              );
            })}
            {fields.length === 0 && (
              <tr>
                <td
                  colSpan={4}
                  className="p-4 text-center text-sm text-muted-foreground"
                >
                  {hasSchemaFields
                    ? 'No outputs configured. Add fields from the schema below.'
                    : 'No outputs yet. Add an output to expose data from the workflow.'}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

      {/* Action buttons */}
      <div className="flex gap-2">
        {availableOptionalFields.length > 0 ? (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                type="button"
                size="sm"
                variant="outline"
                className="flex-1"
              >
                <Plus className="h-4 w-4 mr-2" />
                Add optional field
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start" className="w-56">
              {availableOptionalFields.map((field) => (
                <DropdownMenuItem
                  key={field.name}
                  onClick={() =>
                    append({
                      type: field.name,
                      value: '',
                      typeHint: 'string',
                      valueType: 'immediate',
                    })
                  }
                >
                  <div className="flex flex-col">
                    <span className="font-mono text-sm">{field.name}</span>
                    {field.description && (
                      <span className="text-xs text-muted-foreground">
                        {field.description}
                      </span>
                    )}
                  </div>
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        ) : (
          <Button
            type="button"
            size="sm"
            variant="outline"
            className="flex-1"
            onClick={() =>
              append({
                type: '',
                value: '',
                typeHint: 'string',
                valueType: 'immediate',
              })
            }
          >
            <Plus className="h-4 w-4 mr-2" />
            Add custom output
          </Button>
        )}
      </div>

      <SchemaPreview
        title="Output schema"
        fields={schemaPreview.map((field) => ({
          ...field,
          type: 'string',
          required: schemaFieldMap.get(field.name)?.required ?? true,
        }))}
        emptyLabel="Outputs map into a string-based schema"
        compact
      />
    </div>
  );
}
