import { useContext, useEffect } from 'react';
import { useFormContext, useWatch } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormItem,
  FormLabel,
} from '@/shared/components/ui/form';
import { NodeFormContext } from './NodeFormContext';
import {
  MappingValueInput,
  ValueMode,
} from './InputMappingField/MappingValueInput';

type DelayStepFieldProps = {
  name: string;
};

export function DelayStepField({ name }: DelayStepFieldProps) {
  const form = useFormContext();
  const { nodeId } = useContext(NodeFormContext);
  const stepType = useWatch({ name: 'stepType', control: form.control });

  useEffect(() => {
    if (stepType !== 'Delay') return;
    if (nodeId) return;

    const currentMapping = form.getValues(name) || [];
    if (currentMapping.length === 0) {
      form.setValue(name, [
        {
          type: 'durationMs',
          value: '',
          typeHint: 'number',
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

  if (stepType !== 'Delay') {
    return null;
  }

  const getValue = (fieldName: string) => {
    const field = (inputMapping || []).find(
      (item: any) => item.type === fieldName
    );
    return field?.value ?? '';
  };

  const getValueType = (fieldName: string) => {
    const field = (inputMapping || []).find(
      (item: any) => item.type === fieldName
    );
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
      return;
    }

    form.setValue(
      name,
      [
        ...mapping,
        {
          type: fieldName,
          value,
          typeHint: 'number',
          valueType: valueType || 'immediate',
        },
      ],
      { shouldDirty: true, shouldTouch: true, shouldValidate: true }
    );
  };

  return (
    <div className="space-y-4">
      <div>
        <p className="text-sm font-medium">Delay Configuration</p>
        <p className="text-xs text-muted-foreground">
          Pause workflow execution for a fixed or computed duration.
        </p>
      </div>

      <FormItem>
        <FormLabel>Duration (ms) *</FormLabel>
        <FormDescription>
          Milliseconds to wait before continuing execution.
        </FormDescription>
        <FormControl>
          <MappingValueInput
            value={String(getValue('durationMs'))}
            onChange={(value) => updateField('durationMs', value)}
            valueType={getValueType('durationMs') as ValueMode}
            onValueTypeChange={(valueType) =>
              updateField('durationMs', getValue('durationMs'), valueType)
            }
            fieldType="number"
            fieldName="durationMs"
            placeholder="60000"
          />
        </FormControl>
      </FormItem>
    </div>
  );
}
