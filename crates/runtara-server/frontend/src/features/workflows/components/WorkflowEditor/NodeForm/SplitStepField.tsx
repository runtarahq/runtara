import {
  Fragment,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import { useFieldArray, useFormContext, useWatch } from 'react-hook-form';
import { NodeFormContext } from './NodeFormContext';
import { Button } from '@/shared/components/ui/button';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import { ChevronRight, Plus, Trash2 } from 'lucide-react';
import {
  MappingValueInput,
  ValueMode,
} from './InputMappingField/MappingValueInput';
import { CompositeValueEditor } from './InputMappingField/CompositeValueEditor';
import { ModeToggleButton } from './InputMappingField/ModeToggleButton';
import { VariablePickerModal } from './InputMappingField/VariablePickerModal';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { Switch } from '@/shared/components/ui/switch';
import { SchemaPreview } from '@/features/workflows/components/SchemaPreview';
import {
  buildSchemaFromFields,
  SchemaField,
} from '@/features/workflows/utils/schema';
import type {
  CompositeArrayValue,
  CompositeObjectValue,
} from '@/features/workflows/stores/nodeFormStore';
import type { VariableSuggestion } from './InputMappingValueField/VariableSuggestions';
import { SourceMappingValueField } from './SourceMappingValueField';
import { SchemaFieldsEditor } from '../EditorSidebar/SchemaFieldsEditor';

type SplitStepFieldProps = {
  name: string;
};

type SplitVariableValueType =
  | 'reference'
  | 'immediate'
  | 'composite'
  | 'template';

type SplitVariableField = {
  name?: string;
  value?: unknown;
  type?: string;
  valueType?: SplitVariableValueType;
};

const SPLIT_VARIABLE_TYPES: { label: string; value: string }[] = [
  { label: 'String', value: 'string' },
  { label: 'Number', value: 'number' },
  { label: 'Boolean', value: 'boolean' },
  { label: 'Object', value: 'object' },
  { label: 'Array', value: 'array' },
  { label: 'File', value: 'file' },
];

export function SplitStepField({ name }: SplitStepFieldProps) {
  const form = useFormContext();
  const { previousSteps, inputSchemaFields: workflowInputFields } =
    useContext(NodeFormContext);
  const [editingVariableIndex, setEditingVariableIndex] = useState<
    number | null
  >(null);
  const [structuredPickerIndex, setStructuredPickerIndex] = useState<
    number | null
  >(null);
  const stepType = useWatch({ name: 'stepType', control: form.control });

  // Variables fields (variables to pass to subgraph)
  const {
    fields: variablesFields,
    append: appendVariableField,
    remove: removeVariableField,
  } = useFieldArray({
    name: 'splitVariablesFields',
    control: form.control,
  });

  // Config options
  const splitSequential = useWatch({
    name: 'splitSequential',
    control: form.control,
  });
  const splitDontStopOnFailed = useWatch({
    name: 'splitDontStopOnFailed',
    control: form.control,
  });
  const splitParallelism = useWatch({
    name: 'splitParallelism',
    control: form.control,
  });
  const splitMaxRetries = useWatch({
    name: 'splitMaxRetries',
    control: form.control,
  });
  const splitRetryDelay = useWatch({
    name: 'splitRetryDelay',
    control: form.control,
  });
  const splitTimeout = useWatch({
    name: 'splitTimeout',
    control: form.control,
  });
  const splitAllowNull = useWatch({
    name: 'splitAllowNull',
    control: form.control,
  });
  const splitConvertSingleValue = useWatch({
    name: 'splitConvertSingleValue',
    control: form.control,
  });
  const splitBatchSize = useWatch({
    name: 'splitBatchSize',
    control: form.control,
  });

  const watchedInputSchemaValues = useWatch({
    name: 'splitInputSchemaFields',
    control: form.control,
  });
  const watchedOutputSchemaValues = useWatch({
    name: 'splitOutputSchemaFields',
    control: form.control,
  });
  const watchedVariableValues = useWatch({
    name: 'splitVariablesFields',
    control: form.control,
  });
  const inputSchemaValues = useMemo(
    () => watchedInputSchemaValues || [],
    [watchedInputSchemaValues]
  );
  const outputSchemaValues = useMemo(
    () => watchedOutputSchemaValues || [],
    [watchedOutputSchemaValues]
  );
  const variableValues = useMemo(
    () => watchedVariableValues || [],
    [watchedVariableValues]
  );

  const parseCompositeObjectValue = (value: unknown): CompositeObjectValue => {
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
        // Keep default empty object for invalid JSON
      }
    }
    return {};
  };

  const parseCompositeArrayValue = (value: unknown): CompositeArrayValue => {
    if (Array.isArray(value)) {
      return value as CompositeArrayValue;
    }
    if (typeof value === 'string' && value.trim()) {
      try {
        const parsed = JSON.parse(value);
        if (Array.isArray(parsed)) {
          return parsed as CompositeArrayValue;
        }
      } catch {
        // Keep default empty array for invalid JSON
      }
    }
    return [];
  };

  const getArrayDisplayValue = (value: unknown): string => {
    if (!value) return 'Click to configure...';
    const parsedArray = parseCompositeArrayValue(value);
    return `Composite: ${parsedArray.length} item${parsedArray.length !== 1 ? 's' : ''}`;
  };

  const getObjectDisplayValue = (value: unknown): string => {
    if (!value) return 'Click to configure...';
    const parsedObject = parseCompositeObjectValue(value);
    const fieldCount = Object.keys(parsedObject).length;
    return `Composite: ${fieldCount} field${fieldCount !== 1 ? 's' : ''}`;
  };

  // Build array source suggestions from previous steps
  const arraySuggestions = useMemo(() => {
    const suggestions: { label: string; value: string }[] = [];

    previousSteps.forEach((step) => {
      // Add typed array outputs directly
      step.outputs.forEach((output) => {
        if (output.type === 'array') {
          suggestions.push({
            label: `${step.name}${output.name ? ` → ${output.name}` : ''}`,
            value: output.path,
          });
        }
      });

      // Add a generic ".outputs" fallback only if no typed array outputs were found
      // This allows users to reference the whole outputs when type info is unavailable
      const hasArrayOutput = step.outputs.some(
        (output) => output.type === 'array'
      );
      if (!hasArrayOutput) {
        suggestions.push({
          label: `${step.name} → outputs`,
          value: `steps['${step.id}'].outputs`,
        });
      }
    });

    // Add workflow input fields that are arrays
    if (workflowInputFields && workflowInputFields.length > 0) {
      for (const field of workflowInputFields) {
        if (field.type === 'array' && field.name) {
          suggestions.push({
            label: `data.${field.name} (workflow input)`,
            value: `data.${field.name}`,
          });
        }
      }
    }

    return suggestions;
  }, [previousSteps, workflowInputFields]);

  // Sync schema fields to form values
  const inputSchemaSignature = useMemo(() => {
    return JSON.stringify(inputSchemaValues || []);
  }, [inputSchemaValues]);

  const outputSchemaSignature = useMemo(() => {
    return JSON.stringify(outputSchemaValues || []);
  }, [outputSchemaValues]);

  const lastInputSignature = useRef<string | null>(null);
  const lastOutputSignature = useRef<string | null>(null);

  // Update inputSchema when fields change
  useEffect(() => {
    if (lastInputSignature.current === inputSchemaSignature) return;
    lastInputSignature.current = inputSchemaSignature;

    const schema = buildSchemaFromFields(inputSchemaValues as SchemaField[]);
    const hasFields = Object.keys(schema).length > 0;
    form.setValue('inputSchema', hasFields ? schema : undefined, {
      shouldDirty: true,
    });
  }, [form, inputSchemaSignature, inputSchemaValues]);

  // Update outputSchema when fields change
  useEffect(() => {
    if (lastOutputSignature.current === outputSchemaSignature) return;
    lastOutputSignature.current = outputSchemaSignature;

    const schema = buildSchemaFromFields(outputSchemaValues as SchemaField[]);
    const hasFields = Object.keys(schema).length > 0;
    form.setValue('outputSchema', hasFields ? schema : undefined, {
      shouldDirty: true,
    });
  }, [form, outputSchemaSignature, outputSchemaValues]);

  if (stepType !== 'Split') {
    return null;
  }

  return (
    <div className="space-y-6">
      <SourceMappingValueField
        name={name}
        label="Array Source"
        description="Select the array to iterate over. Each item will be processed by the subgraph."
        suggestions={arraySuggestions}
        placeholder="e.g., steps['fetch'].outputs.items"
      />

      {/* Input Schema (Item Schema) */}
      <div className="space-y-2">
        <p className="text-xs text-muted-foreground">
          Define the structure of each item in the array. This will be available
          as <code className="text-xs bg-muted px-1 rounded">data.*</code>{' '}
          inside the subgraph.
        </p>
        <SchemaFieldsEditor
          label="Item Schema (Input)"
          fields={(inputSchemaValues || []) as any}
          onChange={(fields) =>
            form.setValue('splitInputSchemaFields', fields, {
              shouldDirty: true,
              shouldTouch: true,
            })
          }
          emptyMessage="No fields defined. Add fields to describe the item structure."
          showEnum
        />
        <SchemaPreview
          title="Item input schema"
          schema={buildSchemaFromFields(inputSchemaValues as SchemaField[])}
          emptyLabel="No item schema defined"
        />
      </div>

      {/* Output Schema */}
      <div className="space-y-2">
        <p className="text-xs text-muted-foreground">
          Define what each iteration produces. Results will be collected into an
          array available as{' '}
          <code className="text-xs bg-muted px-1 rounded">
            steps['split-id'].outputs
          </code>
          .
        </p>
        <SchemaFieldsEditor
          label="Output Schema"
          fields={(outputSchemaValues || []) as any}
          onChange={(fields) =>
            form.setValue('splitOutputSchemaFields', fields, {
              shouldDirty: true,
              shouldTouch: true,
            })
          }
          emptyMessage="No output fields defined."
          showEnum
        />
        <SchemaPreview
          title="Iteration output schema"
          schema={buildSchemaFromFields(outputSchemaValues as SchemaField[])}
          emptyLabel="No output schema defined"
        />
      </div>

      {/* Variables (passed to subgraph) */}
      <div className="space-y-2">
        <Label className="text-sm font-medium">Variables</Label>
        <p className="text-xs text-muted-foreground">
          Define variables to pass to each iteration. These will be available as{' '}
          <code className="text-xs bg-muted px-1 rounded">variables.*</code>{' '}
          inside the subgraph.
        </p>
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
              {variablesFields.map((field, index) => {
                const variable = (variableValues[index] ||
                  {}) as SplitVariableField;
                const variableType = variable.type || 'string';
                const isObjectVariable = variableType === 'object';
                const isArrayVariable = variableType === 'array';
                const isStructuredVariable =
                  isObjectVariable || isArrayVariable;
                const isStructuredReference =
                  variable.valueType === 'reference';
                const scalarValue =
                  typeof variable.value === 'string'
                    ? variable.value
                    : variable.value === undefined || variable.value === null
                      ? ''
                      : JSON.stringify(variable.value);
                // Pass template through so MappingValueInput renders its
                // template input and the mode toggle can cycle out of it —
                // clamping it to 'immediate' left the form stuck holding a
                // valueType the UI never displayed.
                const scalarValueType: ValueMode =
                  variable.valueType === 'reference'
                    ? 'reference'
                    : variable.valueType === 'template'
                      ? 'template'
                      : 'immediate';

                return (
                  <Fragment key={field.id}>
                    <tr className="border-b hover:bg-muted/30">
                      <td className="p-2">
                        <div>
                          <Input
                            {...form.register(
                              `splitVariablesFields.${index}.name`
                            )}
                            placeholder="variableName"
                            className="font-mono text-sm border-0 p-1 h-auto focus-visible:ring-0"
                          />
                          {form.getFieldState(
                            `splitVariablesFields.${index}.name`,
                            form.formState
                          ).error?.message && (
                            <p className="text-xs text-destructive mt-1">
                              {
                                form.getFieldState(
                                  `splitVariablesFields.${index}.name`,
                                  form.formState
                                ).error?.message
                              }
                            </p>
                          )}
                        </div>
                      </td>
                      <td className="p-2">
                        {isStructuredVariable ? (
                          <div className="flex items-start gap-2">
                            {isStructuredReference ? (
                              <MappingValueInput
                                value={
                                  typeof variable.value === 'string'
                                    ? variable.value
                                    : ''
                                }
                                onChange={(value) =>
                                  form.setValue(
                                    `splitVariablesFields.${index}.value`,
                                    value,
                                    {
                                      shouldDirty: true,
                                    }
                                  )
                                }
                                valueType="reference"
                                onValueTypeChange={() => {
                                  form.setValue(
                                    `splitVariablesFields.${index}.valueType`,
                                    'composite',
                                    { shouldDirty: true }
                                  );
                                  form.setValue(
                                    `splitVariablesFields.${index}.value`,
                                    isArrayVariable ? [] : {},
                                    { shouldDirty: true }
                                  );
                                  setEditingVariableIndex(index);
                                }}
                                fieldType={variableType}
                                placeholder="Select reference..."
                                hideReferenceToggle
                              />
                            ) : (
                              <button
                                type="button"
                                onClick={() => {
                                  const nextValue = isArrayVariable
                                    ? parseCompositeArrayValue(variable.value)
                                    : parseCompositeObjectValue(variable.value);
                                  form.setValue(
                                    `splitVariablesFields.${index}.valueType`,
                                    'composite',
                                    { shouldDirty: true }
                                  );
                                  form.setValue(
                                    `splitVariablesFields.${index}.value`,
                                    nextValue,
                                    { shouldDirty: true }
                                  );
                                  setEditingVariableIndex((prev) =>
                                    prev === index ? null : index
                                  );
                                }}
                                className="w-full flex items-center justify-between gap-2 px-3 py-2 text-sm border rounded-md bg-muted/30 hover:bg-muted/50 transition-colors text-left"
                              >
                                <span className="text-muted-foreground truncate">
                                  {isArrayVariable
                                    ? getArrayDisplayValue(variable.value)
                                    : getObjectDisplayValue(variable.value)}
                                </span>
                                <ChevronRight className="h-4 w-4 text-muted-foreground shrink-0" />
                              </button>
                            )}
                            <ModeToggleButton
                              mode={
                                isStructuredReference
                                  ? 'reference'
                                  : 'immediate'
                              }
                              onClick={() => {
                                if (isStructuredReference) {
                                  form.setValue(
                                    `splitVariablesFields.${index}.valueType`,
                                    'composite',
                                    { shouldDirty: true }
                                  );
                                  form.setValue(
                                    `splitVariablesFields.${index}.value`,
                                    isArrayVariable ? [] : {},
                                    { shouldDirty: true }
                                  );
                                  setEditingVariableIndex(index);
                                } else {
                                  if (editingVariableIndex === index) {
                                    setEditingVariableIndex(null);
                                  }
                                  setStructuredPickerIndex(index);
                                }
                              }}
                            />
                          </div>
                        ) : (
                          <MappingValueInput
                            value={scalarValue}
                            onChange={(value) =>
                              form.setValue(
                                `splitVariablesFields.${index}.value`,
                                value,
                                {
                                  shouldDirty: true,
                                }
                              )
                            }
                            valueType={scalarValueType}
                            onValueTypeChange={(valueType) =>
                              form.setValue(
                                `splitVariablesFields.${index}.valueType`,
                                valueType,
                                { shouldDirty: true }
                              )
                            }
                            fieldType={variableType}
                            placeholder="Select reference or enter value"
                          />
                        )}
                        {form.getFieldState(
                          `splitVariablesFields.${index}.value`,
                          form.formState
                        ).error?.message && (
                          <p className="text-xs text-destructive mt-1">
                            {
                              form.getFieldState(
                                `splitVariablesFields.${index}.value`,
                                form.formState
                              ).error?.message
                            }
                          </p>
                        )}
                        {/* Visible fallback for valueType (enum) failures —
                            without it a rejected mode silently dead-ends the
                            Save button with no rendered error. */}
                        {form.getFieldState(
                          `splitVariablesFields.${index}.valueType`,
                          form.formState
                        ).error?.message && (
                          <p className="text-xs text-destructive mt-1">
                            {`Unsupported value mode: ${
                              form.getFieldState(
                                `splitVariablesFields.${index}.valueType`,
                                form.formState
                              ).error?.message
                            }`}
                          </p>
                        )}
                      </td>
                      <td className="p-2">
                        <Select
                          value={variableType}
                          onValueChange={(value) => {
                            form.setValue(
                              `splitVariablesFields.${index}.type`,
                              value,
                              {
                                shouldDirty: true,
                              }
                            );
                            if (value !== 'object' && value !== 'array') {
                              if (editingVariableIndex === index) {
                                setEditingVariableIndex(null);
                              }
                              if (structuredPickerIndex === index) {
                                setStructuredPickerIndex(null);
                              }
                              form.setValue(
                                `splitVariablesFields.${index}.valueType`,
                                'immediate',
                                { shouldDirty: true }
                              );
                            } else {
                              form.setValue(
                                `splitVariablesFields.${index}.valueType`,
                                'composite',
                                { shouldDirty: true }
                              );
                              form.setValue(
                                `splitVariablesFields.${index}.value`,
                                value === 'array'
                                  ? parseCompositeArrayValue(variable.value)
                                  : parseCompositeObjectValue(variable.value),
                                { shouldDirty: true }
                              );
                              setStructuredPickerIndex(null);
                              setEditingVariableIndex(index);
                            }
                          }}
                        >
                          <SelectTrigger className="h-7 border-0 focus:ring-0">
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent>
                            {SPLIT_VARIABLE_TYPES.map((type) => (
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
                            if (editingVariableIndex === index) {
                              setEditingVariableIndex(null);
                            }
                            if (structuredPickerIndex === index) {
                              setStructuredPickerIndex(null);
                            }
                            removeVariableField(index);
                          }}
                          className="h-6 w-6 p-0"
                        >
                          <Trash2 className="h-3 w-3" />
                        </Button>
                      </td>
                    </tr>
                    {isStructuredVariable &&
                      !isStructuredReference &&
                      editingVariableIndex === index && (
                        <tr className="hover:bg-transparent">
                          <td colSpan={4} className="p-0 border-t-0">
                            <div className="border-t border-primary/20 bg-muted/20">
                              <CompositeValueEditor
                                value={
                                  isArrayVariable
                                    ? parseCompositeArrayValue(variable.value)
                                    : parseCompositeObjectValue(variable.value)
                                }
                                onChange={(value) => {
                                  form.setValue(
                                    `splitVariablesFields.${index}.valueType`,
                                    'composite',
                                    { shouldDirty: true }
                                  );
                                  form.setValue(
                                    `splitVariablesFields.${index}.value`,
                                    value,
                                    { shouldDirty: true }
                                  );
                                }}
                                onClose={() => setEditingVariableIndex(null)}
                                showModeSwitcher={false}
                              />
                            </div>
                          </td>
                        </tr>
                      )}
                  </Fragment>
                );
              })}
              {variablesFields.length === 0 && (
                <tr>
                  <td
                    colSpan={4}
                    className="p-4 text-center text-sm text-muted-foreground"
                  >
                    No variables defined. Add variables to pass data to
                    iterations.
                  </td>
                </tr>
              )}
            </tbody>
          </table>
          <VariablePickerModal
            open={structuredPickerIndex !== null}
            onOpenChange={(open) => {
              if (!open) {
                setStructuredPickerIndex(null);
              }
            }}
            onSelect={(selected: VariableSuggestion) => {
              if (structuredPickerIndex === null) return;
              form.setValue(
                `splitVariablesFields.${structuredPickerIndex}.valueType`,
                'reference',
                { shouldDirty: true }
              );
              form.setValue(
                `splitVariablesFields.${structuredPickerIndex}.value`,
                selected.value,
                { shouldDirty: true }
              );
              if (editingVariableIndex === structuredPickerIndex) {
                setEditingVariableIndex(null);
              }
              setStructuredPickerIndex(null);
            }}
          />
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() =>
            appendVariableField({
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

      {/* Execution Options */}
      <div className="space-y-4">
        <Label className="text-sm font-medium">Execution Options</Label>

        <div className="grid grid-cols-3 gap-3">
          <div className="space-y-1">
            <Label className="text-sm">Parallelism</Label>
            <Input
              type="number"
              min={0}
              value={splitParallelism ?? ''}
              onChange={(event) =>
                form.setValue(
                  'splitParallelism',
                  event.target.value === ''
                    ? undefined
                    : Number(event.target.value),
                  { shouldDirty: true }
                )
              }
            />
            <p className="text-xs text-muted-foreground">
              Runtime currently warns when this is not 1.
            </p>
          </div>
          <div className="space-y-1">
            <Label className="text-sm">Retries</Label>
            <Input
              type="number"
              min={0}
              value={splitMaxRetries ?? ''}
              onChange={(event) =>
                form.setValue(
                  'splitMaxRetries',
                  event.target.value === ''
                    ? undefined
                    : Number(event.target.value),
                  { shouldDirty: true }
                )
              }
            />
          </div>
          <div className="space-y-1">
            <Label className="text-sm">Retry delay (ms)</Label>
            <Input
              type="number"
              min={0}
              value={splitRetryDelay ?? ''}
              onChange={(event) =>
                form.setValue(
                  'splitRetryDelay',
                  event.target.value === ''
                    ? undefined
                    : Number(event.target.value),
                  { shouldDirty: true }
                )
              }
            />
          </div>
          <div className="space-y-1">
            <Label className="text-sm">Timeout (ms)</Label>
            <Input
              type="number"
              min={0}
              value={splitTimeout ?? ''}
              onChange={(event) =>
                form.setValue(
                  'splitTimeout',
                  event.target.value === ''
                    ? undefined
                    : Number(event.target.value),
                  { shouldDirty: true }
                )
              }
            />
          </div>
          <div className="space-y-1">
            <Label className="text-sm">Batch size</Label>
            <Input
              type="number"
              min={1}
              value={splitBatchSize ?? ''}
              onChange={(event) =>
                form.setValue(
                  'splitBatchSize',
                  event.target.value === ''
                    ? undefined
                    : Number(event.target.value),
                  { shouldDirty: true }
                )
              }
            />
          </div>
        </div>

        <div className="flex items-center justify-between">
          <div className="space-y-0.5">
            <Label className="text-sm">Sequential Execution</Label>
            <p className="text-xs text-muted-foreground">
              Iterations always run one at a time in the current runtime; this flag is
              informational and does not change execution
            </p>
          </div>
          <Switch
            checked={splitSequential ?? false}
            onCheckedChange={(checked) =>
              form.setValue('splitSequential', checked, { shouldDirty: true })
            }
          />
        </div>

        <div className="flex items-center justify-between">
          <div className="space-y-0.5">
            <Label className="text-sm">Continue on Failure</Label>
            <p className="text-xs text-muted-foreground">
              Continue processing remaining items even if some iterations fail
            </p>
          </div>
          <Switch
            checked={splitDontStopOnFailed ?? false}
            onCheckedChange={(checked) =>
              form.setValue('splitDontStopOnFailed', checked, {
                shouldDirty: true,
              })
            }
          />
        </div>

        <div className="flex items-center justify-between">
          <div className="space-y-0.5">
            <Label className="text-sm">Allow Null Input</Label>
            <p className="text-xs text-muted-foreground">
              Treat null input as an empty array.
            </p>
          </div>
          <Switch
            checked={splitAllowNull ?? false}
            onCheckedChange={(checked) =>
              form.setValue('splitAllowNull', checked, { shouldDirty: true })
            }
          />
        </div>

        <div className="flex items-center justify-between">
          <div className="space-y-0.5">
            <Label className="text-sm">Convert Single Value</Label>
            <p className="text-xs text-muted-foreground">
              Wrap a non-array input in a single-item array.
            </p>
          </div>
          <Switch
            checked={splitConvertSingleValue ?? false}
            onCheckedChange={(checked) =>
              form.setValue('splitConvertSingleValue', checked, {
                shouldDirty: true,
              })
            }
          />
        </div>
      </div>
    </div>
  );
}
