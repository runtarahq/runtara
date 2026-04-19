import { useState, useCallback } from 'react';
import { Send, Loader2 } from 'lucide-react';
import { Button } from '@/shared/components/ui/button';
import { WaitingForInputData } from '@/features/workflows/types/chat';
import { parseSchema } from '@/features/workflows/utils/schema';
import { SchemaFormFields } from '@/features/workflows/components/SchemaFormFields';
import {
  isFieldVisible,
  humanizeKey,
} from '@/features/workflows/components/SchemaFormFields/utils';
import { deliverSignal } from '@/features/workflows/queries';
import { useChatStore } from '@/features/workflows/stores/chatStore';

interface ChatFormInputProps {
  waitingForInput: WaitingForInputData;
  instanceId: string;
  token: string;
}

export function ChatFormInput({
  waitingForInput,
  instanceId,
  token,
}: ChatFormInputProps) {
  const schemaFields = parseSchema(waitingForInput.responseSchema);
  const [isSubmitting, setIsSubmitting] = useState(false);
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

  const updateField = useCallback((name: string, value: any) => {
    setFormValues((prev) => ({ ...prev, [name]: value }));
  }, []);

  // Check if all required visible fields are filled
  const isValid = schemaFields
    .filter((f) => f.required !== false && isFieldVisible(f, formValues))
    .every((f) => {
      const val = formValues[f.name];
      if (f.type === 'boolean') return true;
      return val !== '' && val !== undefined && val !== null;
    });

  const handleSubmit = useCallback(async () => {
    if (!isValid || isSubmitting) return;

    // Build payload, coercing types
    const payload: Record<string, any> = {};
    for (const field of schemaFields) {
      if (!isFieldVisible(field, formValues)) continue;
      const val = formValues[field.name];
      if (field.type === 'number' || field.type === 'integer') {
        payload[field.name] = val !== '' ? Number(val) : undefined;
      } else if (field.type === 'boolean') {
        payload[field.name] = Boolean(val);
      } else {
        payload[field.name] = val;
      }
    }

    setIsSubmitting(true);
    const store = useChatStore.getState();

    try {
      await deliverSignal(token, instanceId, {
        signalId: waitingForInput.signalId,
        payload,
      });

      // Add a user message summarizing the submitted form
      const summary = Object.entries(payload)
        .map(([key, val]) => {
          const field = schemaFields.find((f) => f.name === key);
          const label = field?.label || humanizeKey(key);
          const displayVal =
            typeof val === 'boolean' ? (val ? 'Yes' : 'No') : String(val ?? '');
          return `${label}: ${displayVal}`;
        })
        .join(' | ');
      store.addUserMessage(summary);

      // Clear waiting state — SSE stream will deliver the next events
      store.setWaitingForInput(null);
      store.setStatus('streaming');
    } catch (err: unknown) {
      const message =
        err instanceof Error ? err.message : 'Failed to submit form';
      store.setError(message);
    } finally {
      setIsSubmitting(false);
    }
  }, [
    isValid,
    isSubmitting,
    schemaFields,
    formValues,
    token,
    instanceId,
    waitingForInput.signalId,
  ]);

  return (
    <div className="border-t bg-background px-4 py-3">
      {waitingForInput.message && (
        <div className="mb-3 rounded-lg bg-amber-50 dark:bg-amber-900/20 border border-amber-200/60 dark:border-amber-700/40 px-3 py-2 text-xs text-amber-700 dark:text-amber-400">
          {waitingForInput.message}
        </div>
      )}

      <div className="mb-3">
        <SchemaFormFields
          fields={schemaFields}
          rawSchema={
            waitingForInput.responseSchema as Record<string, any> | undefined
          }
          formValues={formValues}
          onChange={updateField}
          disabled={isSubmitting}
        />
      </div>

      <Button
        onClick={handleSubmit}
        disabled={!isValid || isSubmitting}
        className="w-full"
        size="sm"
      >
        {isSubmitting ? (
          <Loader2 className="h-4 w-4 mr-2 animate-spin" />
        ) : (
          <Send className="h-4 w-4 mr-2" />
        )}
        Submit
      </Button>
    </div>
  );
}
