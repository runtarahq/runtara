import { useContext } from 'react';
import { useFormContext } from 'react-hook-form';
import {
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/shared/components/ui/form';
import { MappingValueInput } from './InputMappingField/MappingValueInput';
import { NodeFormContext } from './NodeFormContext';
import {
  MAX_RUN_LABEL_LENGTH,
  RUN_LABEL_HELP,
} from '@/features/workflows/utils/run-label';

export function RunLabelField() {
  const form = useFormContext();
  const { isInsideSplit, isInsideWhileLoop, isInsideWaitScope } =
    useContext(NodeFormContext);
  const valueError = form.getFieldState('runLabel.value', form.formState).error;
  const labelError = form.getFieldState('runLabel', form.formState).error;
  if (isInsideSplit || isInsideWhileLoop || isInsideWaitScope) return null;

  return (
    <FormField
      control={form.control}
      name="runLabel"
      render={({ field }) => {
        const label = field.value ?? { valueType: 'immediate', value: '' };
        const labelLength = (
          typeof label.value === 'string' ? label.value : ''
        ).replace(/^ +| +$/g, '').length;
        return (
          <FormItem>
            <FormLabel>Run label (optional)</FormLabel>
            <MappingValueInput
              fieldName="runLabel"
              fieldType="string"
              value={label.value}
              valueType={label.valueType}
              modes={['immediate', 'reference', 'template']}
              placeholder="Order/123 [processed]"
              onChange={(value) => field.onChange({ ...label, value })}
              onValueTypeChange={(valueType) =>
                field.onChange({ valueType, value: '' })
              }
              defaultValue={label.default}
              onDefaultValueChange={(value) =>
                field.onChange({ ...label, default: value })
              }
            />
            <p className="text-xs text-muted-foreground">{RUN_LABEL_HELP}</p>
            {label.valueType === 'immediate' && (
              <p className="text-xs text-muted-foreground">
                {labelLength}/{MAX_RUN_LABEL_LENGTH}
                {labelLength > MAX_RUN_LABEL_LENGTH && ' — will be truncated'}
              </p>
            )}
            {labelError?.message && <FormMessage />}
            {valueError && (
              <p className="text-sm text-destructive">{valueError.message}</p>
            )}
          </FormItem>
        );
      }}
    />
  );
}
