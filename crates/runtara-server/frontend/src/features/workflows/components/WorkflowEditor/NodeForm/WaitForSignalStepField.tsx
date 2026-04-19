import { useContext, useEffect, useState } from 'react';
import { useFormContext, useWatch } from 'react-hook-form';
import {
  FormControl,
  FormItem,
  FormLabel,
  FormDescription,
} from '@/shared/components/ui/form';
import { Button } from '@/shared/components/ui/button';
import { ChevronDown, ChevronRight } from 'lucide-react';
import { NodeFormContext } from './NodeFormContext';
import {
  MappingValueInput,
  ValueMode,
} from './InputMappingField/MappingValueInput';
import {
  SchemaFieldsEditor,
  type SchemaField as EditorSchemaField,
} from '../EditorSidebar/SchemaFieldsEditor';

type WaitForSignalStepFieldProps = {
  name: string;
};

export function WaitForSignalStepField({ name }: WaitForSignalStepFieldProps) {
  const form = useFormContext();
  const { nodeId } = useContext(NodeFormContext);
  const stepType = useWatch({ name: 'stepType', control: form.control });
  const [showAdvanced, setShowAdvanced] = useState(false);

  // Initialize default inputMapping entries for new nodes
  useEffect(() => {
    if (stepType !== 'WaitForSignal') return;
    if (nodeId) return; // Don't reset in edit mode

    const currentMapping = form.getValues(name) || [];
    if (currentMapping.length === 0) {
      form.setValue(name, [
        {
          type: 'responseSchema',
          value: [],
          typeHint: 'json',
          valueType: 'immediate',
        },
        {
          type: 'timeoutMs',
          value: '',
          typeHint: 'number',
          valueType: 'immediate',
        },
        {
          type: 'pollIntervalMs',
          value: '1000',
          typeHint: 'number',
          valueType: 'immediate',
        },
      ]);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stepType, nodeId]);

  // Watch inputMapping for reactivity
  const inputMapping = useWatch({
    name,
    control: form.control,
    defaultValue: [],
  });

  if (stepType !== 'WaitForSignal') {
    return null;
  }

  // Helpers to read/write inputMapping entries
  const getValue = (fieldName: string) => {
    const mapping = inputMapping || [];
    const field = mapping.find((item: any) => item.type === fieldName);
    return field?.value ?? '';
  };

  const getValueType = (fieldName: string) => {
    const mapping = inputMapping || [];
    const field = mapping.find((item: any) => item.type === fieldName);
    return field?.valueType || 'immediate';
  };

  const updateField = (
    fieldName: string,
    value: any,
    valueType?: ValueMode
  ) => {
    const mapping = form.getValues(name) || [];
    const fieldIndex = mapping.findIndex(
      (item: any) => item.type === fieldName
    );

    if (fieldIndex >= 0) {
      form.setValue(`${name}.${fieldIndex}.value`, value, {
        shouldDirty: true,
        shouldTouch: true,
        shouldValidate: true,
      });
      if (valueType !== undefined) {
        form.setValue(`${name}.${fieldIndex}.valueType`, valueType, {
          shouldDirty: true,
          shouldTouch: true,
          shouldValidate: true,
        });
      }
    } else {
      form.setValue(
        name,
        [
          ...mapping,
          {
            type: fieldName,
            value,
            typeHint: fieldName === 'responseSchema' ? 'json' : 'number',
            valueType: valueType || 'immediate',
          },
        ],
        { shouldDirty: true, shouldTouch: true, shouldValidate: true }
      );
    }
  };

  // Response schema: stored as SchemaField[] in inputMapping
  const responseSchemaFields: EditorSchemaField[] = Array.isArray(
    getValue('responseSchema')
  )
    ? getValue('responseSchema')
    : [];

  return (
    <div className="space-y-4">
      <div>
        <p className="text-sm font-medium">Wait For Signal Configuration</p>
        <p className="text-xs text-muted-foreground">
          Suspends execution until a human provides input. Define the response
          schema to control what fields the human sees.
        </p>
      </div>

      {/* Response Schema */}
      <div className="space-y-2">
        <SchemaFieldsEditor
          label="Response Schema"
          fields={responseSchemaFields}
          onChange={(fields) => updateField('responseSchema', fields)}
          emptyMessage="No response fields defined. Add fields to define what the human will fill in."
        />
      </div>

      {/* Timeout */}
      <FormItem>
        <FormLabel>Timeout (ms)</FormLabel>
        <FormDescription>
          Optional timeout in milliseconds. If no signal is received within this
          duration, the step fails.
        </FormDescription>
        <FormControl>
          <MappingValueInput
            value={String(getValue('timeoutMs'))}
            onChange={(value) => updateField('timeoutMs', value)}
            valueType={getValueType('timeoutMs') as ValueMode}
            onValueTypeChange={(vt) =>
              updateField('timeoutMs', getValue('timeoutMs'), vt)
            }
            fieldType="string"
            placeholder="e.g. 86400000 (24 hours)"
          />
        </FormControl>
      </FormItem>

      {/* Advanced Settings */}
      <div>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="px-0 text-xs text-muted-foreground"
          onClick={() => setShowAdvanced(!showAdvanced)}
        >
          {showAdvanced ? (
            <ChevronDown className="h-3 w-3 mr-1" />
          ) : (
            <ChevronRight className="h-3 w-3 mr-1" />
          )}
          Advanced Settings
        </Button>

        {showAdvanced && (
          <div className="mt-2 space-y-4">
            {/* Poll Interval */}
            <FormItem>
              <FormLabel>Poll Interval (ms)</FormLabel>
              <FormDescription>
                How often to check for the signal (default: 1000ms). Lower
                values mean faster response but more server load.
              </FormDescription>
              <FormControl>
                <MappingValueInput
                  value={String(getValue('pollIntervalMs'))}
                  onChange={(value) => updateField('pollIntervalMs', value)}
                  valueType={getValueType('pollIntervalMs') as ValueMode}
                  onValueTypeChange={(vt) =>
                    updateField(
                      'pollIntervalMs',
                      getValue('pollIntervalMs'),
                      vt
                    )
                  }
                  fieldType="string"
                  placeholder="1000"
                  hideReferenceToggle
                />
              </FormControl>
            </FormItem>
          </div>
        )}
      </div>

      <div className="rounded-md border border-blue-500/50 bg-blue-500/10 p-3 text-sm">
        <p className="text-blue-600 dark:text-blue-400">
          Wire this step as an AI Agent tool to enable human-in-the-loop
          workflows. When the LLM calls this tool, execution suspends until a
          human provides input matching the response schema.
        </p>
      </div>
    </div>
  );
}
