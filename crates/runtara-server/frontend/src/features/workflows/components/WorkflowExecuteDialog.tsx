import { useState, useEffect } from 'react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/shared/components/ui/dialog';
import { Button } from '@/shared/components/ui/button';
import { validateWorkflowStartInputsWithRust } from '@/features/workflows/utils/rust-workflow-validation';
import { FormRenderer, type FormAnalysisResult } from '@/shared/forms';
import {
  initialWorkflowFormValues,
  useWorkflowFormDefinition,
} from '@/features/workflows/utils/form-schema-adapter';

type WorkflowExecuteDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  workflowName?: string;
  inputSchema?: any;
  onExecute: (inputData: Record<string, any>) => void;
  isSubmitting?: boolean;
  serverError?: string | null;
};

export function WorkflowExecuteDialog({
  open,
  onOpenChange,
  workflowName,
  inputSchema,
  onExecute,
  isSubmitting = false,
  serverError,
}: WorkflowExecuteDialogProps) {
  const {
    definition,
    loading: formLoading,
    error: formError,
  } = useWorkflowFormDefinition(inputSchema);
  const fieldNames = Object.keys(definition.fields);

  const [inputData, setInputData] = useState<Record<string, any>>(() =>
    initialWorkflowFormValues(definition)
  );
  const [formAnalysis, setFormAnalysis] = useState<FormAnalysisResult | null>(
    null
  );
  const [rustValidationError, setRustValidationError] = useState<string | null>(
    null
  );
  const [isRustValidating, setIsRustValidating] = useState(false);
  const [submitAttempt, setSubmitAttempt] = useState(0);

  // Reset input data and validation errors when input schema changes
  useEffect(() => {
    setInputData(initialWorkflowFormValues(definition));
    setFormAnalysis(null);
    setRustValidationError(null);
  }, [definition]);

  // Reset input data and validation errors when dialog opens
  useEffect(() => {
    if (open) {
      setInputData(initialWorkflowFormValues(definition));
      setFormAnalysis(null);
      setRustValidationError(null);
    }
  }, [open, definition]);

  const handleExecute = async () => {
    setSubmitAttempt((attempt) => attempt + 1);
    setRustValidationError(null);
    if (
      formLoading ||
      formError ||
      (fieldNames.length > 0 && !formAnalysis?.valid)
    ) {
      return;
    }
    // Only include fields that are set and defined
    const filteredData: Record<string, any> = {};
    for (const [key, value] of Object.entries(inputData)) {
      const field = definition.fields[key];
      if (
        value !== undefined &&
        formAnalysis?.fields[key]?.visible !== false &&
        !(value === '' && field?.required === false)
      ) {
        filteredData[key] = value;
      }
    }

    const backendInputs = {
      data: filteredData,
      variables: {},
    };

    setIsRustValidating(true);
    try {
      const rustValidation = await validateWorkflowStartInputsWithRust(
        inputSchema ?? {},
        backendInputs
      );

      if (rustValidation.status === 'invalid') {
        setRustValidationError(
          rustValidation.errors.join('; ') || rustValidation.message
        );
        return;
      }
    } finally {
      setIsRustValidating(false);
    }

    onExecute(filteredData);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl max-h-[80vh] overflow-y-auto">
        <DialogHeader>
          <DialogTitle>
            Execute Workflow{workflowName ? `: ${workflowName}` : ''}
          </DialogTitle>
          <DialogDescription>
            This workflow requires input data. Please provide the required
            fields below.
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4 py-4">
          {formLoading ? (
            <p className="text-sm text-muted-foreground">Preparing form…</p>
          ) : formError ? (
            <div className="rounded-md bg-destructive/10 p-3 text-sm text-destructive">
              {formError}
            </div>
          ) : fieldNames.length === 0 ? (
            <p className="text-sm text-muted-foreground">
              No input fields required.
            </p>
          ) : (
            <FormRenderer
              definition={definition}
              value={inputData}
              onChange={(next) => {
                setInputData(next);
                setRustValidationError(null);
              }}
              disabled={isSubmitting || isRustValidating}
              onAnalysisChange={setFormAnalysis}
              submitAttempt={submitAttempt}
            />
          )}
          {serverError && (
            <div className="rounded-md bg-destructive/10 p-3 text-sm text-destructive">
              {serverError}
            </div>
          )}
          {rustValidationError && (
            <div className="rounded-md bg-destructive/10 p-3 text-sm text-destructive">
              {rustValidationError}
            </div>
          )}
        </div>

        <DialogFooter>
          <Button
            variant="outline"
            onClick={() => onOpenChange(false)}
            disabled={isSubmitting}
          >
            Cancel
          </Button>
          <Button
            onClick={handleExecute}
            disabled={
              isSubmitting ||
              isRustValidating ||
              formLoading ||
              Boolean(formError) ||
              formAnalysis?.wasmAvailable === false
            }
          >
            {isSubmitting || isRustValidating ? 'Executing...' : 'Execute'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
