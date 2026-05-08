import { useState } from 'react';
import { Loader2 } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { SchemaFormFields } from '@/features/workflows/components/SchemaFormFields';
import { isFieldVisible } from '@/features/workflows/components/SchemaFormFields/utils';
import { parseSchema } from '@/features/workflows/utils/schema';

interface ActionFormProps {
  inputSchema?: Record<string, any> | null;
  disabled?: boolean;
  submitLabel?: string;
  emptySchemaMessage?: string;
  onSubmit: (payload: Record<string, any>) => void;
}

export function ActionForm({
  inputSchema,
  disabled = false,
  submitLabel = 'Submit Response',
  emptySchemaMessage = 'No response schema defined. Submit an empty response to continue.',
  onSubmit,
}: ActionFormProps) {
  const schemaFields = parseSchema(inputSchema);
  const [formValues, setFormValues] = useState<Record<string, any>>(() => {
    const defaults: Record<string, any> = {};
    for (const field of schemaFields) {
      if (field.defaultValue !== undefined) {
        defaults[field.name] = field.defaultValue;
      } else if (field.type === 'boolean') {
        defaults[field.name] = false;
      } else {
        defaults[field.name] = '';
      }
    }
    return defaults;
  });

  const updateField = (name: string, value: any) => {
    setFormValues((prev) => ({ ...prev, [name]: value }));
  };

  const isValid = schemaFields
    .filter((field) => field.required !== false && isFieldVisible(field, formValues))
    .every((field) => {
      const value = formValues[field.name];
      if (field.type === 'boolean') return true;
      return value !== '' && value !== undefined && value !== null;
    });

  const handleSubmit = () => {
    const payload: Record<string, any> = {};
    for (const field of schemaFields) {
      if (!isFieldVisible(field, formValues)) continue;
      const value = formValues[field.name];
      if (field.type === 'number' || field.type === 'integer') {
        payload[field.name] = value !== '' ? Number(value) : undefined;
      } else if (field.type === 'boolean') {
        payload[field.name] = Boolean(value);
      } else {
        payload[field.name] = value;
      }
    }
    onSubmit(payload);
  };

  return (
    <div className="space-y-4">
      {schemaFields.length > 0 ? (
        <SchemaFormFields
          fields={schemaFields}
          rawSchema={inputSchema ?? undefined}
          formValues={formValues}
          onChange={updateField}
          disabled={disabled}
        />
      ) : (
        <p className="text-sm text-muted-foreground">{emptySchemaMessage}</p>
      )}
      <Button
        size="sm"
        className="w-full"
        onClick={handleSubmit}
        disabled={disabled || !isValid}
      >
        {disabled ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
        {submitLabel}
      </Button>
    </div>
  );
}
