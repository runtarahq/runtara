import { useEffect, useState } from 'react';
import { Loader2 } from 'lucide-react';

import { Button } from '@/shared/components/ui/button';
import {
  analyzeFormWithRust,
  FormRenderer,
  type FormAnalysisResult,
} from '@/shared/forms';
import {
  initialWorkflowFormValues,
  useWorkflowFormDefinition,
} from '@/features/workflows/utils/form-schema-adapter';

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
  const { definition, loading, error } = useWorkflowFormDefinition(inputSchema);
  const [formValues, setFormValues] = useState<Record<string, unknown>>(() =>
    initialWorkflowFormValues(definition)
  );
  const [analysis, setAnalysis] = useState<FormAnalysisResult | null>(null);
  const [submitAttempt, setSubmitAttempt] = useState(0);
  const hasFields = Object.keys(definition.fields).length > 0;

  useEffect(() => {
    setFormValues(initialWorkflowFormValues(definition));
    setAnalysis(null);
  }, [definition]);

  const handleSubmit = async () => {
    const submissionAnalysis = hasFields
      ? await analyzeFormWithRust(definition, formValues)
      : analysis;
    if (submissionAnalysis) setAnalysis(submissionAnalysis);
    setSubmitAttempt((attempt) => attempt + 1);
    if (hasFields && !submissionAnalysis?.valid) return;
    const payload = Object.fromEntries(
      Object.keys(definition.fields)
        .filter((name) => submissionAnalysis?.fields[name]?.visible !== false)
        .map((name) => [name, formValues[name]])
    );
    onSubmit(payload);
  };

  return (
    <div className="space-y-4">
      {loading ? (
        <p className="text-sm text-muted-foreground">Preparing form…</p>
      ) : error ? (
        <p className="text-sm text-destructive">{error}</p>
      ) : hasFields ? (
        <FormRenderer
          definition={definition}
          value={formValues}
          onChange={setFormValues}
          disabled={disabled}
          onAnalysisChange={setAnalysis}
          submitAttempt={submitAttempt}
        />
      ) : (
        <p className="text-sm text-muted-foreground">{emptySchemaMessage}</p>
      )}
      <Button
        type="button"
        size="sm"
        className="w-full"
        onClick={handleSubmit}
        disabled={
          disabled ||
          loading ||
          Boolean(error) ||
          analysis?.wasmAvailable === false
        }
      >
        {disabled ? <Loader2 className="mr-2 h-4 w-4 animate-spin" /> : null}
        {submitLabel}
      </Button>
    </div>
  );
}
