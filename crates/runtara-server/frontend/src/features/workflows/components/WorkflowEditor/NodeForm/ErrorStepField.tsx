import { useContext, useEffect } from 'react';
import { useFormContext, useFormState, useWatch } from 'react-hook-form';
import {
  FormControl,
  FormItem,
  FormLabel,
  FormMessage,
  FormDescription,
} from '@/shared/components/ui/form';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { Textarea } from '@/shared/components/ui/textarea';
import { Input } from '@/shared/components/ui/input';
import { NodeFormContext } from './NodeFormContext';
import { ValueMode } from './InputMappingField/MappingValueInput';
import { MappingObjectField } from './InputMappingField/MappingObjectField';

type ErrorStepFieldProps = {
  name: string;
};

export function ErrorStepField({ name }: ErrorStepFieldProps) {
  const form = useFormContext();
  const { nodeId } = useContext(NodeFormContext);
  const stepType = useWatch({ name: 'stepType', control: form.control });

  // Initialize Error step fields when first created
  useEffect(() => {
    if (stepType !== 'Error') return;
    if (nodeId) return; // Don't reset in edit mode

    // Set default values if not already set
    const currentMapping = form.getValues(name) || [];
    if (currentMapping.length === 0) {
      form.setValue(name, [
        {
          type: 'code',
          value: '',
          typeHint: 'string',
          valueType: 'immediate',
        },
        {
          type: 'message',
          value: '',
          typeHint: 'string',
          valueType: 'immediate',
        },
        {
          type: 'category',
          value: 'permanent',
          typeHint: 'string',
          valueType: 'immediate',
        },
        {
          type: 'severity',
          value: 'error',
          typeHint: 'string',
          valueType: 'immediate',
        },
      ]);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stepType, nodeId]);

  // Watch the inputMapping array to make fields reactive
  const inputMapping = useWatch({
    name,
    control: form.control,
    defaultValue: [],
  });

  // Early return after all hooks are called
  if (stepType !== 'Error') {
    return null;
  }

  // Helper to get current value from inputMapping array
  const getValue = (fieldName: string) => {
    const mapping = inputMapping || [];
    const field = mapping.find((item: any) => item.type === fieldName);
    return field?.value || '';
  };

  // Helper to update a field in the inputMapping array
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
      // Update existing field
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
      // Add new field
      form.setValue(
        name,
        [
          ...mapping,
          {
            type: fieldName,
            value,
            typeHint: fieldName === 'context' ? 'json' : 'string',
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
        <p className="text-sm font-medium">Error Configuration</p>
        <p className="text-xs text-muted-foreground">
          Configure the error details that will be thrown when this step
          executes.
        </p>
      </div>

      {/* Error Code */}
      <FormItem>
        <FormLabel>Error Code *</FormLabel>
        <FormDescription>
          Machine-readable error code (e.g., "CREDIT_LIMIT_EXCEEDED",
          "INVALID_ACCOUNT"). Used verbatim — references are not resolved.
        </FormDescription>
        <FormControl>
          <Input
            value={getValue('code')}
            onChange={(event) =>
              updateField('code', event.target.value, 'immediate')
            }
            placeholder="Enter error code..."
          />
        </FormControl>
        <ErrorFieldMessage fieldName="code" name={name} label="Error Code" />
      </FormItem>

      {/* Error Message */}
      <FormItem>
        <FormLabel>Error Message *</FormLabel>
        <FormDescription>
          Human-readable error message, emitted verbatim — references and
          templates are not resolved here. Put dynamic values in Context
          below.
        </FormDescription>
        <FormControl>
          <Textarea
            value={getValue('message')}
            onChange={(event) =>
              updateField('message', event.target.value, 'immediate')
            }
            placeholder="Enter error message..."
          />
        </FormControl>
        <ErrorFieldMessage
          fieldName="message"
          name={name}
          label="Error Message"
        />
      </FormItem>

      {/* Error Category */}
      <FormItem>
        <FormLabel>Category</FormLabel>
        <FormDescription>
          Determines retry behavior: "transient" for recoverable errors
          (network, timeout), "permanent" for non-recoverable errors
          (validation, authorization)
        </FormDescription>
        <Select
          value={getValue('category') || 'permanent'}
          onValueChange={(value) => updateField('category', value)}
        >
          <FormControl>
            <SelectTrigger>
              <SelectValue placeholder="Select category" />
            </SelectTrigger>
          </FormControl>
          <SelectContent>
            <SelectItem value="transient">
              Transient (Retry likely to succeed)
            </SelectItem>
            <SelectItem value="permanent">Permanent (Don't retry)</SelectItem>
          </SelectContent>
        </Select>
        <FormMessage />
      </FormItem>

      {/* Error Severity */}
      <FormItem>
        <FormLabel>Severity</FormLabel>
        <FormDescription>Error severity for logging/alerting</FormDescription>
        <Select
          value={getValue('severity') || 'error'}
          onValueChange={(value) => updateField('severity', value)}
        >
          <FormControl>
            <SelectTrigger>
              <SelectValue placeholder="Select severity" />
            </SelectTrigger>
          </FormControl>
          <SelectContent>
            <SelectItem value="info">Info (Informational)</SelectItem>
            <SelectItem value="warning">
              Warning (Degraded but functional)
            </SelectItem>
            <SelectItem value="error">Error (Operation failed)</SelectItem>
            <SelectItem value="critical">
              Critical (System-level failure)
            </SelectItem>
          </SelectContent>
        </Select>
        <FormMessage />
      </FormItem>

      <FormItem>
        <FormLabel>Context</FormLabel>
        <FormDescription>
          Optional DSL input-mapping object attached to the structured error.
        </FormDescription>
        <MappingObjectField
          value={getValue('context')}
          onChange={(next) => updateField('context', next, 'composite')}
          jsonPlaceholder='{"caseId": {"valueType": "reference", "value": "data.caseId"}}'
        />
      </FormItem>

      <div className="rounded-md border border-blue-500/50 bg-blue-500/10 p-3 text-sm">
        <p className="text-blue-600 dark:text-blue-400">
          Error steps are typically added as error handlers on other steps. They
          define structured error information that will be thrown when
          triggered.
        </p>
      </div>
    </div>
  );
}

function ErrorFieldMessage({
  fieldName,
  name,
  label,
}: {
  fieldName: string;
  name: string;
  label: string;
}) {
  const form = useFormContext();
  const { isSubmitted } = useFormState({ control: form.control });
  const inputMapping = useWatch({
    name,
    control: form.control,
    defaultValue: [],
  });

  if (!isSubmitted) return null;

  const entry = inputMapping?.find((item: any) => item.type === fieldName);
  // Don't show error for reference/composite values — they resolve at runtime
  if (entry?.valueType === 'reference' || entry?.valueType === 'composite')
    return null;
  const isEmpty =
    !entry ||
    entry.value === undefined ||
    entry.value === null ||
    (typeof entry.value === 'string' && entry.value.trim() === '');

  if (!isEmpty) return null;
  return (
    <p className="text-[0.8rem] font-medium text-destructive">
      {label} is required.
    </p>
  );
}
