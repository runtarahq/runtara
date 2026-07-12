import { useMemo, useState } from 'react';
import { Loader2 } from 'lucide-react';

import { Button } from '@/shared/components/ui/button';
import { FormRenderer, type FormAnalysisResult } from '@/shared/forms';
import {
  initialWorkflowFormValues,
  workflowSchemaToFormDefinition,
} from '@/features/workflows/utils/form-schema-adapter';
import { parseSchema } from '@/features/workflows/utils/schema';

interface ActionFormProps {
  inputSchema?: Record<string, unknown> | null;
  disabled?: boolean;
  submitLabel?: string;
  emptySchemaMessage?: string;
  onSubmit: (payload: Record<string, unknown>) => void;
}

export function ActionForm({
  inputSchema,
  disabled = false,
  submitLabel = 'Submit Response',
  emptySchemaMessage = 'No response schema defined. Submit an empty response to continue.',
  onSubmit,
}: ActionFormProps) {
  const definition = useMemo(
    () => workflowSchemaToFormDefinition(parseSchema(inputSchema)),
    [inputSchema]
  );
  const [formValues, setFormValues] = useState<Record<string, unknown>>(() =>
    initialWorkflowFormValues(definition)
  );
  const [analysis, setAnalysis] = useState<FormAnalysisResult | null>(null);
  const hasFields = Object.keys(definition.fields).length > 0;

  const handleSubmit = () => {
    if (hasFields && !analysis?.valid) return;
    const payload = Object.fromEntries(
      Object.keys(definition.fields)
        .filter((name) => analysis?.fields[name]?.visible !== false)
        .map((name) => [name, formValues[name]])
    );
    onSubmit(payload);
  };

  return (
    <div className="space-y-4">
      {hasFields ? (
        <FormRenderer
          definition={definition}
          value={formValues}
          onChange={setFormValues}
          disabled={disabled}
          onAnalysisChange={setAnalysis}
        />
      ) : (
        <p className="text-sm text-muted-foreground">{emptySchemaMessage}</p>
      )}
      <Button
        type="button"
        size="sm"
        className="w-full"
        onClick={handleSubmit}
        disabled={disabled || (hasFields && !analysis?.valid)}
      >
        {disabled ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
        {submitLabel}
      </Button>
    </div>
  );
}
