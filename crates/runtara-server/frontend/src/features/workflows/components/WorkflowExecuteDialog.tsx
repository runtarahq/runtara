import { useState, useEffect, useMemo } from 'react';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/shared/components/ui/dialog';
import { Button } from '@/shared/components/ui/button';
import { parseSchema, SchemaField } from '@/features/workflows/utils/schema';
import {
  SchemaInputForm,
  SchemaInputFormChangeAction,
} from '@/shared/components/SchemaInputForm';
import { validateWorkflowStartInputsWithRust } from '@/features/workflows/utils/rust-workflow-validation';

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
  const fields = useMemo(() => parseSchema(inputSchema), [inputSchema]);

  // Fields with defaults (and booleans, which render as checkboxes) start
  // "set"; key presence in inputData marks a field as touched.
  const getInitialData = (schemaFields: SchemaField[]) => {
    const initial: Record<string, any> = {};
    schemaFields.forEach((field) => {
      if (field.defaultValue !== undefined) {
        initial[field.name] = field.defaultValue;
      } else if (field.type === 'boolean') {
        initial[field.name] = false;
      }
      // All other types without defaults start as undefined (not in the record)
    });
    return initial;
  };

  const [inputData, setInputData] = useState<Record<string, any>>(() =>
    getInitialData(fields)
  );

  const [validationErrors, setValidationErrors] = useState<
    Record<string, string>
  >({});
  const [rustValidationError, setRustValidationError] = useState<string | null>(
    null
  );
  const [isRustValidating, setIsRustValidating] = useState(false);

  // Reset input data and validation errors when input schema changes
  useEffect(() => {
    setInputData(getInitialData(fields));
    setValidationErrors({});
    setRustValidationError(null);
  }, [fields]);

  // Reset input data and validation errors when dialog opens
  useEffect(() => {
    if (open) {
      setInputData(getInitialData(fields));
      setValidationErrors({});
      setRustValidationError(null);
    }
  }, [open, fields]);

  const handleDataChange = (
    next: Record<string, any>,
    changedField: string,
    action: SchemaInputFormChangeAction
  ) => {
    setInputData(next);
    if (action === 'set') {
      setRustValidationError(null);
    }
    // Clear validation error for this field when user makes changes
    if (validationErrors[changedField]) {
      setValidationErrors((prev) => {
        const { [changedField]: _removed, ...rest } = prev;
        void _removed;
        return rest;
      });
    }
  };

  const getValidationErrors = (): Record<string, string> => {
    const errors: Record<string, string> = {};
    fields.forEach((field) => {
      if (field.required) {
        const value = inputData[field.name];
        if (
          !(field.name in inputData) ||
          value === undefined ||
          value === null
        ) {
          errors[field.name] = `${field.name} is required`;
        }
      }
    });
    return errors;
  };

  const handleExecute = async () => {
    const errors = getValidationErrors();
    setValidationErrors(errors);
    setRustValidationError(null);
    if (Object.keys(errors).length > 0) {
      return;
    }
    // Only include fields that are set and defined
    const filteredData: Record<string, any> = {};
    for (const [key, value] of Object.entries(inputData)) {
      if (value !== undefined) {
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
          {fields.length === 0 ? (
            <p className="text-sm text-muted-foreground">
              No input fields required.
            </p>
          ) : (
            <SchemaInputForm
              inputSchema={inputSchema}
              value={inputData}
              onChange={handleDataChange}
              errors={validationErrors}
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
            disabled={isSubmitting || isRustValidating}
          >
            {isSubmitting || isRustValidating ? 'Executing...' : 'Execute'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
