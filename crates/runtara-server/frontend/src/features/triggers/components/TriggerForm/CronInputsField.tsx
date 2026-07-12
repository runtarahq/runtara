import { useMemo, useState } from 'react';
import { X } from 'lucide-react';
import { useController, useWatch } from 'react-hook-form';
import { FormLabel } from '@/shared/components/ui/form';
import { Button } from '@/shared/components/ui/button';
import { Textarea } from '@/shared/components/ui/textarea';
import { useWorkflowFormDefinition } from '@/features/workflows/utils/form-schema-adapter';
import { FormRenderer } from '@/shared/forms';
import {
  analyzeStaticInputs,
  buildStaticInputsText,
  staticInputsError,
} from '@/features/triggers/utils/trigger-configuration';

interface CronInputsFieldProps {
  label: string;
  disabled?: boolean;
  /** Workflows loaded by the trigger pages; used to resolve the selected workflow's input schema. */
  workflows?: { id: string; inputSchema?: any }[];
}

/**
 * Static input envelope editor for CRON triggers, round-tripping
 * `configuration.inputs` through the `cronInputs` form value (a JSON
 * string).
 *
 * When the selected workflow has a non-empty input schema, the schema is
 * rendered as a structured form (the same renderer as the workflow Run
 * dialog) that writes the `{"data": {...}}` envelope into `cronInputs`. An
 * "Advanced (JSON)" toggle keeps the validated textarea for envelopes the
 * form cannot represent (variables overrides, extra keys); both surfaces
 * edit the same underlying value, and anything the form does not cover is
 * preserved verbatim (with a warning) rather than dropped.
 */
export function CronInputsField({
  label,
  disabled,
  workflows,
}: CronInputsFieldProps) {
  const { field, fieldState } = useController({ name: 'cronInputs' });
  const triggerTypeWatch = useWatch({ name: 'triggerType' });
  const workflowIdWatch = useWatch({ name: 'workflowId' });
  // User preference; only honored while the structured form can represent
  // the current value (invalid JSON always falls back to the textarea).
  const [advancedMode, setAdvancedMode] = useState(false);

  const value = typeof field.value === 'string' ? field.value : '';

  const selectedWorkflow = useMemo(
    () =>
      workflows?.find((workflow) => workflow.id === workflowIdWatch) ?? null,
    [workflows, workflowIdWatch]
  );

  const {
    definition: formDefinition,
    loading: formLoading,
    error: formNormalizationError,
  } = useWorkflowFormDefinition(selectedWorkflow?.inputSchema);

  const schemaFieldNames = useMemo(
    () => Object.keys(formDefinition.fields),
    [formDefinition]
  );

  const analysis = useMemo(
    () => analyzeStaticInputs(value, schemaFieldNames),
    [value, schemaFieldNames]
  );

  if (triggerTypeWatch !== 'CRON') {
    return null;
  }

  // Validate as the user types; the form schema also blocks save while invalid.
  const error = staticInputsError(value) ?? fieldState.error?.message ?? null;

  // The structured form needs a schema and (when disabled) the plain
  // textarea keeps its established disabled rendering.
  const structuredAvailable =
    schemaFieldNames.length > 0 &&
    !disabled &&
    !formLoading &&
    !formNormalizationError;
  const structuredActive =
    structuredAvailable && analysis.representable && !advancedMode;

  const unrepresentedKeyLabels = analysis.representable
    ? [
        ...analysis.unrepresentedEnvelopeKeys,
        ...analysis.unrepresentedDataKeys.map((key) => `data.${key}`),
      ]
    : [];
  const structuredValue = Object.fromEntries(
    schemaFieldNames
      .filter((name) => Object.hasOwn(analysis.data, name))
      .map((name) => [name, analysis.data[name]])
  );

  return (
    <div className="space-y-2">
      <div className="flex items-center justify-between gap-2">
        <FormLabel>{label}</FormLabel>
        {structuredAvailable && (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-6 px-2 text-xs"
            disabled={!structuredActive && !analysis.representable}
            onClick={() => setAdvancedMode(structuredActive)}
          >
            {structuredActive ? 'Advanced (JSON)' : 'Structured form'}
          </Button>
        )}
      </div>

      {structuredActive && analysis.representable ? (
        <>
          {unrepresentedKeyLabels.length > 0 && (
            <p className="rounded-md border border-amber-300 bg-amber-50 px-3 py-2 text-[0.7rem] leading-tight text-amber-900">
              The current JSON contains keys this form does not edit (
              {unrepresentedKeyLabels.join(', ')}). They are kept as-is when you
              change fields here; use Advanced (JSON) to edit them.
            </p>
          )}
          <div className="rounded-md border border-input p-3">
            <FormRenderer
              definition={formDefinition}
              value={structuredValue}
              onChange={(nextData) =>
                field.onChange(
                  buildStaticInputsText(value, nextData, schemaFieldNames)
                )
              }
              fieldAnnotations={Object.fromEntries(
                Object.entries(formDefinition.fields)
                  .filter(
                    ([name, formField]) =>
                      !formField.required &&
                      Object.hasOwn(structuredValue, name)
                  )
                  .map(([name, formField]) => [
                    name,
                    <Button
                      key={name}
                      type="button"
                      variant="ghost"
                      size="sm"
                      className="h-6 px-2 text-xs text-muted-foreground"
                      aria-label={`Clear ${formField.label ?? name}`}
                      onClick={() => {
                        const { [name]: _removed, ...nextData } =
                          structuredValue;
                        void _removed;
                        field.onChange(
                          buildStaticInputsText(
                            value,
                            nextData,
                            schemaFieldNames
                          )
                        );
                      }}
                    >
                      <X className="mr-1 h-3.5 w-3.5" />
                      Clear
                    </Button>,
                  ])
              )}
            />
          </div>
          <p className="text-[0.7rem] leading-tight text-muted-foreground">
            Optional. Values are sent as the workflow input envelope (
            {'{"data": {...}}'}) on each fire. Leave fields unset to start the
            workflow with an empty input.
          </p>
        </>
      ) : (
        <>
          <Textarea
            name={field.name}
            ref={field.ref}
            value={value}
            onChange={field.onChange}
            onBlur={field.onBlur}
            disabled={disabled}
            rows={6}
            placeholder='{"data": {}, "variables": {}}'
            className="font-mono text-xs"
            aria-invalid={!!error}
          />
          <p className="text-[0.7rem] leading-tight text-muted-foreground">
            Optional. Sent as the workflow input envelope on each fire, e.g.{' '}
            {'{"data": {...}, "variables": {...}}'}. Leave blank to start the
            workflow with an empty input.
          </p>
          {structuredAvailable && !analysis.representable && (
            <p className="text-[0.7rem] leading-tight text-muted-foreground">
              {analysis.reason === 'invalid-json'
                ? 'Fix the JSON to switch back to the structured form.'
                : 'The "data" key is not a JSON object, so the structured form cannot edit it.'}
            </p>
          )}
        </>
      )}

      {error && (
        <p className="text-[0.8rem] font-medium text-destructive">{error}</p>
      )}
      {formNormalizationError && (
        <p className="text-[0.8rem] font-medium text-destructive">
          {formNormalizationError}
        </p>
      )}
    </div>
  );
}
