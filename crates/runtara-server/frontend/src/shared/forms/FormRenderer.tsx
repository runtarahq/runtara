import { useEffect, useId, useRef, useState } from 'react';
import { AlertCircle, Loader2 } from 'lucide-react';

import {
  Alert,
  AlertDescription,
  AlertTitle,
} from '@/shared/components/ui/alert';
import { Label } from '@/shared/components/ui/label';

import { FieldControl } from './FieldControl';
import { FormSection } from './FormSection';
import { analyzeFormWithRust } from './rust-form-validation';
import type {
  FormAnalysisResult,
  FormDefinition,
  FormField,
  FormIssue,
  FormSectionDefinition,
} from './types';

interface FormRendererProps {
  definition: FormDefinition;
  value: Record<string, unknown>;
  onChange: (value: Record<string, unknown>) => void;
  disabled?: boolean;
  className?: string;
  onAnalysisChange?: (analysis: FormAnalysisResult) => void;
}

function orderedFields(definition: FormDefinition): [string, FormField][] {
  return Object.entries(definition.fields).sort(
    ([leftName, left], [rightName, right]) => {
      const order = (left.order ?? 0) - (right.order ?? 0);
      return order || leftName.localeCompare(rightName);
    }
  );
}

function orderedSections(
  definition: FormDefinition
): (FormSectionDefinition | undefined)[] {
  return [
    undefined,
    ...(definition.sections ?? []).slice().sort((left, right) => {
      const order = (left.order ?? 0) - (right.order ?? 0);
      return order || left.label.localeCompare(right.label);
    }),
  ];
}

function issuesForField(issues: FormIssue[], name: string): FormIssue[] {
  const path = `data.${name}`;
  return issues.filter(
    (issue) => issue.path === path || issue.path.startsWith(`${path}.`)
  );
}

export function FormRenderer({
  definition,
  value,
  onChange,
  disabled = false,
  className,
  onAnalysisChange,
}: FormRendererProps) {
  const formId = useId().replace(/:/g, '');
  const request = useRef(0);
  const analysisCallback = useRef(onAnalysisChange);
  const [analysis, setAnalysis] = useState<FormAnalysisResult | null>(null);

  useEffect(() => {
    analysisCallback.current = onAnalysisChange;
  }, [onAnalysisChange]);

  useEffect(() => {
    const current = ++request.current;
    void analyzeFormWithRust(definition, value).then((next) => {
      if (request.current !== current) return;
      setAnalysis(next);
      analysisCallback.current?.(next);
    });
  }, [definition, value]);

  if (!analysis) {
    return (
      <div
        className="flex items-center gap-2 text-sm text-muted-foreground"
        role="status"
      >
        <Loader2 className="h-4 w-4 animate-spin" />
        Preparing form…
      </div>
    );
  }

  if (!analysis.wasmAvailable) {
    return (
      <Alert variant="destructive">
        <AlertCircle className="h-4 w-4" />
        <AlertTitle>Form unavailable</AlertTitle>
        <AlertDescription>
          The shared validation engine could not start. Reload the page before
          editing or submitting this form.
        </AlertDescription>
      </Alert>
    );
  }

  const fields = orderedFields(definition);
  return (
    <div className={className ?? 'space-y-4'}>
      {orderedSections(definition).map((section) => {
        const sectionFields = fields.filter(
          ([, field]) => field.section === section?.id
        );
        const visibleFields = sectionFields.filter(
          ([name]) => analysis.fields[name]?.visible !== false
        );
        if (visibleFields.length === 0) return null;

        return (
          <FormSection key={section?.id ?? '__default'} section={section}>
            {visibleFields.map(([name, field]) => {
              const state = analysis.fields[name] ?? {
                visible: true,
                enabled: true,
                required: Boolean(field.required),
              };
              const fieldIssues = issuesForField(analysis.issues, name);
              const inputId = `${formId}-${name.replace(/\./g, '-')}`;
              const fieldDisabled =
                disabled || !state.enabled || field.access === 'read';
              return (
                <div key={name} className="space-y-1.5" data-field={name}>
                  <Label htmlFor={inputId} className="text-sm font-medium">
                    {field.label ?? name.replace(/_/g, ' ')}
                    {state.required && (
                      <span className="ml-0.5 text-destructive">*</span>
                    )}
                  </Label>
                  {field.description && (
                    <p className="text-xs text-muted-foreground">
                      {field.description}
                    </p>
                  )}
                  <FieldControl
                    id={inputId}
                    field={field}
                    value={value[name]}
                    disabled={fieldDisabled}
                    invalid={fieldIssues.length > 0}
                    onChange={(next) => onChange({ ...value, [name]: next })}
                  />
                  {fieldIssues.map((issue) => (
                    <p
                      key={`${issue.code}-${issue.path}`}
                      className="text-xs text-destructive"
                    >
                      {issue.message}
                    </p>
                  ))}
                </div>
              );
            })}
          </FormSection>
        );
      })}
    </div>
  );
}
