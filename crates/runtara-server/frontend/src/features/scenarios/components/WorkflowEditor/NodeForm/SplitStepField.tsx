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
import { SchemaPreview } from '@/features/scenarios/components/SchemaPreview';
import {
  buildSchemaFromFields,
  SchemaField,
} from '@/features/scenarios/utils/schema';
import type {
  CompositeArrayValue,
  CompositeObjectValue,
} from '@/features/scenarios/stores/nodeFormStore';
import type { VariableSuggestion } from './InputMappingValueField/VariableSuggestions';

type SplitStepFieldProps = {
  name: string;
};

type SplitVariableValueType = 'reference' | 'immediate' | 'composite';

type SplitVariableField = {
  name?: string;
  value?: unknown;
  type?: string;
  valueType?: SplitVariableValueType;
};

const SUPPORTED_TYPES: { label: string; value: string }[] = [
  { label: 'String', value: 'string' },
  { label: 'Number', value: 'number' },
  { label: 'Boolean', value: 'boolean' },
  { label: 'Object', value: 'object' },
  { label: 'Array', value: 'array' },
];

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
  const { previousSteps, inputSchemaFields: scenarioInputFields } =
    useContext(NodeFormContext);
  const inputMapping = useWatch({ name, control: form.control });
  const [editingVariableIndex, setEditingVariableIndex] = useState<
    number | null
  >(null);
  const [structuredPickerIndex, setStructuredPickerIndex] = useState<
    number | null
  >(null);
  const stepType = useWatch({ name: 'stepType', control: form.control });

  // Input schema fields (item schema for each iteration)
  const {
    fields: inputSchemaFields,
    append: appendInputField,
    remove: removeInputField,
  } = useFieldArray({
    name: 'splitInputSchemaFields',
    control: form.control,
  });

  // Output schema fields (what each iteration produces)
  const {
    fields: outputSchemaFields,
    append: appendOutputField,
    remove: removeOutputField,
  } = useFieldArray({
    name: 'splitOutputSchemaFields',
    control: form.control,
  });

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
  const splitParallelism = useWatch({
    name: 'splitParallelism',
    control: form.control,
  });
  const splitSequential = useWatch({
    name: 'splitSequential',
    control: form.control,
  });
  const splitDontStopOnFailed = useWatch({
    name: 'splitDontStopOnFailed',
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

    // Add scenario input fields that are arrays
    if (scenarioInputFields && scenarioInputFields.length > 0) {
      for (const field of scenarioInputFields) {
        if (field.type === 'array' && field.name) {
          suggestions.push({
            label: `data.${field.name} (scenario input)`,
            value: `data.${field.name}`,
          });
        }
      }
    }

    return suggestions;
  }, [previousSteps, scenarioInputFields]);

  // Sync schema fields to form values
  const inputSchemaSignature = useMemo(() => {
    return JSON.stringify(
      (inputSchemaValues as SchemaField[]).map((f) => ({
        name: f?.name || '',
        type: f?.type || 'string',
      }))
    );
  }, [inputSchemaValues]);

  const outputSchemaSignature = useMemo(() => {
    return JSON.stringify(
      (outputSchemaValues as SchemaField[]).map((f) => ({
        name: f?.name || '',
        type: f?.type || 'string',
      }))
    );
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
      {/* Array Source Selection */}
      <div className="space-y-2">
        <Label className="text-sm font-medium">Array Source</Label>
        <p className="text-xs text-muted-foreground">
          Select the array to iterate over. Each item will be processed by the
          subgraph.
        </p>
        <Select
          value={inputMapping?.[0]?.value || ''}
          onValueChange={(value) => {
            form.setValue(
              name,
              [
                {
                  type: 'value',
                  value,
                  typeHint: 'auto',
                  valueType: 'reference',
                },
              ],
              { shouldDirty: true }
            );
          }}
        >
          <SelectTrigger>
            <SelectValue placeholder="Select array source..." />
          </SelectTrigger>
          <SelectContent>
            {arraySuggestions.map((suggestion) => (
              <SelectItem key={suggestion.value} value={suggestion.value}>
                {suggestion.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <p className="text-xs text-muted-foreground">
          Or enter a custom path:{' '}
          <Input
            className="mt-1"
            placeholder="e.g., steps['fetch'].outputs.items"
            value={inputMapping?.[0]?.value || ''}
            onChange={(e) => {
              form.setValue(
                name,
                [
                  {
                    type: 'value',
                    value: e.target.value,
                    typeHint: 'auto',
                    valueType: 'reference',
                  },
                ],
                { shouldDirty: true }
              );
            }}
          />
        </p>
      </div>

      {/* Input Schema (Item Schema) */}
      <div className="space-y-2">
        <Label className="text-sm font-medium">Item Schema (Input)</Label>
        <p className="text-xs text-muted-foreground">
          Define the structure of each item in the array. This will be available
          as <code className="text-xs bg-muted px-1 rounded">data.*</code>{' '}
          inside the subgraph.
        </p>
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
                <th className="w-16 text-center p-2 text-sm font-medium text-muted-foreground">
                  Actions
                </th>
              </tr>
            </thead>
            <tbody>
              {inputSchemaFields.map((field, index) => (
                <tr key={field.id} className="border-b hover:bg-muted/30">
                  <td className="p-2">
                    <Input
                      {...form.register(`splitInputSchemaFields.${index}.name`)}
                      placeholder="fieldName"
                      className="font-mono text-sm border-0 p-1 h-auto focus-visible:ring-0"
                    />
                  </td>
                  <td className="p-2">
                    <Select
                      value={
                        form.getValues(
                          `splitInputSchemaFields.${index}.type`
                        ) || 'string'
                      }
                      onValueChange={(value) =>
                        form.setValue(
                          `splitInputSchemaFields.${index}.type`,
                          value,
                          { shouldDirty: true }
                        )
                      }
                    >
                      <SelectTrigger className="h-7 border-0 focus:ring-0">
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
                  <td className="w-16 text-center p-2">
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      onClick={() => removeInputField(index)}
                      className="h-6 w-6 p-0"
                    >
                      <Trash2 className="h-3 w-3" />
                    </Button>
                  </td>
                </tr>
              ))}
              {inputSchemaFields.length === 0 && (
                <tr>
                  <td
                    colSpan={3}
                    className="p-4 text-center text-sm text-muted-foreground"
                  >
                    No fields defined. Add fields to describe the item
                    structure.
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => appendInputField({ name: '', type: 'string' })}
          className="w-full"
        >
          <Plus className="h-4 w-4 mr-2" />
          Add Input Field
        </Button>
        <SchemaPreview
          title="Item input schema"
          schema={buildSchemaFromFields(inputSchemaValues as SchemaField[])}
          emptyLabel="No item schema defined"
        />
      </div>

      {/* Output Schema */}
      <div className="space-y-2">
        <Label className="text-sm font-medium">Output Schema</Label>
        <p className="text-xs text-muted-foreground">
          Define what each iteration produces. Results will be collected into an
          array available as{' '}
          <code className="text-xs bg-muted px-1 rounded">
            steps['split-id'].outputs
          </code>
          .
        </p>
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
                <th className="w-16 text-center p-2 text-sm font-medium text-muted-foreground">
                  Actions
                </th>
              </tr>
            </thead>
            <tbody>
              {outputSchemaFields.map((field, index) => (
                <tr key={field.id} className="border-b hover:bg-muted/30">
                  <td className="p-2">
                    <Input
                      {...form.register(
                        `splitOutputSchemaFields.${index}.name`
                      )}
                      placeholder="resultField"
                      className="font-mono text-sm border-0 p-1 h-auto focus-visible:ring-0"
                    />
                  </td>
                  <td className="p-2">
                    <Select
                      value={
                        form.getValues(
                          `splitOutputSchemaFields.${index}.type`
                        ) || 'string'
                      }
                      onValueChange={(value) =>
                        form.setValue(
                          `splitOutputSchemaFields.${index}.type`,
                          value,
                          { shouldDirty: true }
                        )
                      }
                    >
                      <SelectTrigger className="h-7 border-0 focus:ring-0">
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
                  <td className="w-16 text-center p-2">
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      onClick={() => removeOutputField(index)}
                      className="h-6 w-6 p-0"
                    >
                      <Trash2 className="h-3 w-3" />
                    </Button>
                  </td>
                </tr>
              ))}
              {outputSchemaFields.length === 0 && (
                <tr>
                  <td
                    colSpan={3}
                    className="p-4 text-center text-sm text-muted-foreground"
                  >
                    No output fields defined.
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => appendOutputField({ name: '', type: 'string' })}
          className="w-full"
        >
          <Plus className="h-4 w-4 mr-2" />
          Add Output Field
        </Button>
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
                const scalarValueType: ValueMode =
                  variable.valueType === 'reference'
                    ? 'reference'
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

        <div className="flex items-center justify-between">
          <div className="space-y-0.5">
            <Label className="text-sm">Sequential Execution</Label>
            <p className="text-xs text-muted-foreground">
              Execute iterations one at a time instead of in parallel
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

        <div className="space-y-2">
          <Label className="text-sm">Parallelism</Label>
          <p className="text-xs text-muted-foreground">
            Maximum concurrent iterations (0 = unlimited)
          </p>
          <Input
            type="number"
            min={0}
            value={splitParallelism ?? 0}
            onChange={(e) =>
              form.setValue('splitParallelism', parseInt(e.target.value) || 0, {
                shouldDirty: true,
              })
            }
            className="w-24"
          />
        </div>
      </div>
    </div>
  );
}
