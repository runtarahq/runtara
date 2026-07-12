import { useEffect, useId, useRef, useState, type ReactNode } from 'react';
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
import { useResolvedOptions } from './use-resolved-options';
import type {
  FormAnalysisResult,
  FormDefinition,
  FormField,
  FormFrameContract,
  FormIssue,
  FormSectionDefinition,
} from './types';

export interface FormRendererProps {
  definition: FormDefinition;
  value: Record<string, unknown>;
  onChange: (value: Record<string, unknown>) => void;
  disabled?: boolean;
  className?: string;
  onAnalysisChange?: (analysis: FormAnalysisResult) => void;
  fieldAnnotations?: Record<string, ReactNode>;
  /** Increment only at an explicit submit boundary to focus the first issue. */
  submitAttempt?: number;
  /** Domain-owned commit, clear, and option-resolution behavior. */
  frame?: FormFrameContract;
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
  fieldAnnotations,
  submitAttempt = 0,
  frame,
}: FormRendererProps) {
  const formId = useId().replace(/:/g, '');
  const root = useRef<HTMLDivElement>(null);
  const request = useRef(0);
  const focusedAttempt = useRef(0);
  const analysisCallback = useRef(onAnalysisChange);
  const [analysis, setAnalysis] = useState<FormAnalysisResult | null>(null);
  const definitionJson = JSON.stringify(definition);
  const valueJson = JSON.stringify(value);
  const resolvedOptions = useResolvedOptions(
    definition,
    value,
    frame?.resolveOptions
  );

  useEffect(() => {
    analysisCallback.current = onAnalysisChange;
  }, [onAnalysisChange]);

  useEffect(() => {
    const current = ++request.current;
    const definitionSnapshot = JSON.parse(definitionJson) as FormDefinition;
    const valueSnapshot = JSON.parse(valueJson) as Record<string, unknown>;
    void analyzeFormWithRust(definitionSnapshot, valueSnapshot).then((next) => {
      if (request.current !== current) return;
      setAnalysis(next);
      analysisCallback.current?.(next);
    });
  }, [definitionJson, valueJson]);

  useEffect(() => {
    if (
      submitAttempt <= focusedAttempt.current ||
      !analysis ||
      analysis.valid
    ) {
      return;
    }
    focusedAttempt.current = submitAttempt;
    const invalid = root.current?.querySelector<HTMLElement>(
      '[data-field] [aria-invalid="true"]'
    );
    const focusTarget = invalid?.matches(
      'input, button, select, textarea, [tabindex]'
    )
      ? invalid
      : invalid?.querySelector<HTMLElement>(
          'input, button, select, textarea, [tabindex]'
        );
    focusTarget?.focus();
  }, [analysis, submitAttempt]);

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
    <div ref={root} className={className ?? 'space-y-4'}>
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
                  {fieldAnnotations?.[name]}
                  <FieldControl
                    id={inputId}
                    field={field}
                    value={value[name]}
                    disabled={fieldDisabled}
                    invalid={fieldIssues.length > 0}
                    options={resolvedOptions.options[name]}
                    optionsLoading={resolvedOptions.loading.has(name)}
                    onChange={(next) => {
                      const nextData = { ...value, [name]: next };
                      if (frame?.commitField) {
                        frame.commitField({
                          fieldName: name,
                          field,
                          value: next,
                          previousData: value,
                          nextData,
                        });
                      } else {
                        onChange(nextData);
                      }
                    }}
                  />
                  {resolvedOptions.errors[name] && (
                    <p className="text-xs text-destructive" role="alert">
                      {resolvedOptions.errors[name]}
                    </p>
                  )}
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
