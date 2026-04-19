import { useState } from 'react';
import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from '@/shared/components/ui/card';
import { Button } from '@/shared/components/ui/button';
import { Badge } from '@/shared/components/ui/badge';
import { Loader2, MessageSquare, Wrench } from 'lucide-react';
import { type PendingInput } from '@/features/workflows/queries';
import { parseSchema } from '@/features/workflows/utils/schema';
import { SchemaFormFields } from '@/features/workflows/components/SchemaFormFields';
import { isFieldVisible } from '@/features/workflows/components/SchemaFormFields/utils';

interface HumanInputCardProps {
  pendingInput: PendingInput;
  onSubmit: (signalId: string, payload: Record<string, any>) => void;
  isSubmitting: boolean;
}

export function HumanInputCard({
  pendingInput,
  onSubmit,
  isSubmitting,
}: HumanInputCardProps) {
  const schemaFields = parseSchema(pendingInput.responseSchema);
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

  const handleSubmit = () => {
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
    onSubmit(pendingInput.signalId, payload);
  };

  const updateField = (name: string, value: any) => {
    setFormValues((prev) => ({ ...prev, [name]: value }));
  };

  // Check if all required visible fields are filled
  const isValid = schemaFields
    .filter((f) => f.required !== false && isFieldVisible(f, formValues))
    .every((f) => {
      const val = formValues[f.name];
      if (f.type === 'boolean') return true;
      return val !== '' && val !== undefined && val !== null;
    });

  return (
    <Card className="border-amber-500/50 bg-amber-500/5">
      <CardHeader className="pb-3">
        <CardTitle className="text-sm font-medium flex items-center gap-2">
          <MessageSquare className="h-4 w-4 text-amber-600" />
          Human Input Required
        </CardTitle>
        <div className="flex items-center gap-2 text-xs text-muted-foreground">
          <Badge variant="outline" className="text-xs gap-1">
            <Wrench className="h-3 w-3" />
            {pendingInput.toolName}
          </Badge>
          <span>Iteration {pendingInput.iteration}</span>
        </div>
      </CardHeader>
      <CardContent className="space-y-4">
        {/* AI Agent's message */}
        {pendingInput.message && (
          <div className="rounded-md bg-muted/50 p-3 text-sm">
            {pendingInput.message}
          </div>
        )}

        {/* Dynamic form fields from responseSchema */}
        {schemaFields.length > 0 ? (
          <SchemaFormFields
            fields={schemaFields}
            rawSchema={pendingInput.responseSchema}
            formValues={formValues}
            onChange={updateField}
            disabled={isSubmitting}
          />
        ) : (
          <p className="text-sm text-muted-foreground">
            No response schema defined. Submit an empty response to continue.
          </p>
        )}

        <Button
          size="sm"
          className="w-full"
          onClick={handleSubmit}
          disabled={isSubmitting || !isValid}
        >
          {isSubmitting ? (
            <Loader2 className="h-4 w-4 mr-2 animate-spin" />
          ) : null}
          Submit Response
        </Button>
      </CardContent>
    </Card>
  );
}
