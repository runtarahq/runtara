import { useContext, useEffect } from 'react';
import { useFormContext, useWatch } from 'react-hook-form';
import {
  FormControl,
  FormItem,
  FormLabel,
  FormDescription,
} from '@/shared/components/ui/form';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { NodeFormContext } from './NodeFormContext';
import {
  MappingValueInput,
  ValueMode,
} from './InputMappingField/MappingValueInput';

type LogStepFieldProps = {
  name: string;
};

export function LogStepField({ name }: LogStepFieldProps) {
  const form = useFormContext();
  const { nodeId } = useContext(NodeFormContext);
  const stepType = useWatch({ name: 'stepType', control: form.control });

  // Initialize Log step fields when first created
  useEffect(() => {
    if (stepType !== 'Log') return;
    if (nodeId) return; // Don't reset in edit mode

    const currentMapping = form.getValues(name) || [];
    if (currentMapping.length === 0) {
      form.setValue(name, [
        {
          type: 'message',
          value: '',
          typeHint: 'string',
          valueType: 'immediate',
        },
        {
          type: 'level',
          value: 'info',
          typeHint: 'string',
          valueType: 'immediate',
        },
      ]);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stepType, nodeId]);

  const inputMapping = useWatch({
    name,
    control: form.control,
    defaultValue: [],
  });

  if (stepType !== 'Log') {
    return null;
  }

  const getValue = (fieldName: string) => {
    const mapping = inputMapping || [];
    const field = mapping.find((item: any) => item.type === fieldName);
    return field?.value || '';
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
            typeHint: 'string',
            valueType: valueType || 'immediate',
          },
        ],
        {
          shouldDirty: true,
          shouldTouch: true,
          shouldValidate: true,
        }
      );
    }
  };

  return (
    <div className="space-y-4">
      <div>
        <p className="text-sm font-medium">Log Configuration</p>
        <p className="text-xs text-muted-foreground">
          Emit a log event during workflow execution.
        </p>
      </div>

      {/* Log Message */}
      <FormItem>
        <FormLabel>Message *</FormLabel>
        <FormDescription>The log message to emit</FormDescription>
        <FormControl>
          <MappingValueInput
            value={getValue('message')}
            onChange={(value) => updateField('message', value)}
            valueType={getValueType('message') as ValueMode}
            onValueTypeChange={(valueType) =>
              updateField('message', getValue('message'), valueType)
            }
            fieldType="textarea"
            placeholder="Enter log message..."
          />
        </FormControl>
      </FormItem>

      {/* Log Level */}
      <FormItem>
        <FormLabel>Level</FormLabel>
        <FormDescription>Log severity level</FormDescription>
        <Select
          value={getValue('level') || 'info'}
          onValueChange={(value) => updateField('level', value)}
        >
          <FormControl>
            <SelectTrigger>
              <SelectValue placeholder="Select level" />
            </SelectTrigger>
          </FormControl>
          <SelectContent>
            <SelectItem value="debug">Debug</SelectItem>
            <SelectItem value="info">Info</SelectItem>
            <SelectItem value="warn">Warn</SelectItem>
            <SelectItem value="error">Error</SelectItem>
          </SelectContent>
        </Select>
      </FormItem>
    </div>
  );
}
