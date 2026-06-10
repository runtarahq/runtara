import { useContext, useEffect, useState } from 'react';
import { useFormContext, useWatch } from 'react-hook-form';
import {
  FormControl,
  FormItem,
  FormLabel,
  FormDescription,
} from '@/shared/components/ui/form';
import { Input } from '@/shared/components/ui/input';
import { Textarea } from '@/shared/components/ui/textarea';
import { Button } from '@/shared/components/ui/button';
import { ChevronDown, ChevronRight, ListTree } from 'lucide-react';
import { NodeFormContext } from './NodeFormContext';
import { useWorkflowStore } from '@/features/workflows/stores/workflowStore.ts';
import { NODE_TYPES } from '@/features/workflows/config/workflow.ts';
import {
  MappingValueInput,
  ValueMode,
} from './InputMappingField/MappingValueInput';
import { MappingObjectField } from './InputMappingField/MappingObjectField';
import {
  SchemaFieldsEditor,
  type SchemaField as EditorSchemaField,
} from '../EditorSidebar/SchemaFieldsEditor';

type WaitForSignalStepFieldProps = {
  name: string;
};

export function WaitForSignalStepField({ name }: WaitForSignalStepFieldProps) {
  const form = useFormContext();
  const { nodeId } = useContext(NodeFormContext);
  const stepType = useWatch({ name: 'stepType', control: form.control });
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [onWaitText, setOnWaitText] = useState('');
  const [onWaitError, setOnWaitError] = useState<string | null>(null);
  // Defensive JSON editing is kept only for an onWait that failed to
  // normalize into a container at load (empty/malformed graph left in step
  // data). Latched per node so the textarea doesn't vanish mid-edit when the
  // user clears it.
  const [showOnWaitJson, setShowOnWaitJson] = useState(false);

  // A WaitForSignal step with an onWait graph is rendered as a container
  // node; its onWait flow is edited visually (timeline/canvas) and the
  // container is the single source of truth.
  const isOnWaitContainer = useWorkflowStore((state) => {
    if (!nodeId) return false;
    const node = state.nodes.find((n) => n.id === nodeId);
    return node?.type === NODE_TYPES.ContainerNode;
  });
  const convertNodeToContainer = useWorkflowStore(
    (state) => state.convertNodeToContainer
  );

  // Initialize default inputMapping entries for new nodes
  useEffect(() => {
    if (stepType !== 'WaitForSignal') return;
    if (nodeId) return; // Don't reset in edit mode

    const currentMapping = form.getValues(name) || [];
    if (currentMapping.length === 0) {
      form.setValue(name, [
        {
          type: 'responseSchema',
          value: [],
          typeHint: 'json',
          valueType: 'immediate',
        },
        {
          type: 'timeoutMs',
          value: '',
          typeHint: 'number',
          valueType: 'immediate',
        },
        {
          type: 'pollIntervalMs',
          value: '1000',
          typeHint: 'number',
          valueType: 'immediate',
        },
      ]);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stepType, nodeId]);

  useEffect(() => {
    const currentOnWait = form.getValues('onWait');
    setOnWaitText(currentOnWait ? JSON.stringify(currentOnWait, null, 2) : '');
    setOnWaitError(null);
    // Container-mode steps never carry a raw onWait in node data (the load
    // path strips it), so a present value means normalization was skipped.
    setShowOnWaitJson(Boolean(currentOnWait));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stepType, nodeId]);

  // Watch inputMapping for reactivity
  const inputMapping = useWatch({
    name,
    control: form.control,
    defaultValue: [],
  });

  if (stepType !== 'WaitForSignal') {
    return null;
  }

  // Helpers to read/write inputMapping entries
  const getValue = (fieldName: string) => {
    const mapping = inputMapping || [];
    const field = mapping.find((item: any) => item.type === fieldName);
    return field?.value ?? '';
  };

  const getValueType = (fieldName: string) => {
    const mapping = inputMapping || [];
    const field = mapping.find((item: any) => item.type === fieldName);
    return field?.valueType || 'immediate';
  };

  const updateField = (
    fieldName: string,
    value: any,
    valueType?: ValueMode
  ) => {
    const mapping = form.getValues(name) || [];
    const fieldIndex = mapping.findIndex(
      (item: any) => item.type === fieldName
    );

    if (fieldIndex >= 0) {
      form.setValue(`${name}.${fieldIndex}.value`, value, {
        shouldDirty: true,
        shouldTouch: true,
        shouldValidate: true,
      });
      if (valueType !== undefined) {
        form.setValue(`${name}.${fieldIndex}.valueType`, valueType, {
          shouldDirty: true,
          shouldTouch: true,
          shouldValidate: true,
        });
      }
    } else {
      const typeHint =
        fieldName === 'responseSchema' ||
        fieldName === 'actionCorrelation' ||
        fieldName === 'actionContext'
          ? 'json'
          : fieldName === 'actionKey'
            ? 'string'
            : 'number';
      form.setValue(
        name,
        [
          ...mapping,
          {
            type: fieldName,
            value,
            typeHint,
            valueType: valueType || 'immediate',
          },
        ],
        { shouldDirty: true, shouldTouch: true, shouldValidate: true }
      );
    }
  };

  const updateOnWait = (value: string) => {
    setOnWaitText(value);

    if (!value.trim()) {
      setOnWaitError(null);
      form.setValue('onWait', undefined, {
        shouldDirty: true,
        shouldTouch: true,
        shouldValidate: true,
      });
      return;
    }

    try {
      const parsed = JSON.parse(value);
      if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
        setOnWaitError('onWait must be an execution graph object.');
        return;
      }
      setOnWaitError(null);
      form.setValue('onWait', parsed, {
        shouldDirty: true,
        shouldTouch: true,
        shouldValidate: true,
      });
    } catch (error) {
      setOnWaitError(
        error instanceof Error ? error.message : 'Invalid JSON object.'
      );
    }
  };

  // Response schema: stored as SchemaField[] in inputMapping
  const responseSchemaFields: EditorSchemaField[] = Array.isArray(
    getValue('responseSchema')
  )
    ? getValue('responseSchema')
    : [];

  return (
    <div className="space-y-4">
      <div>
        <p className="text-sm font-medium">Wait For Signal Configuration</p>
        <p className="text-xs text-muted-foreground">
          Suspends execution until a human provides input. Define the response
          schema to control what fields the human sees.
        </p>
      </div>

      {/* Response Schema */}
      <div className="space-y-2">
        <SchemaFieldsEditor
          label="Response Schema"
          fields={responseSchemaFields}
          onChange={(fields) => updateField('responseSchema', fields)}
          emptyMessage="No response fields defined. Add fields to define what the human will fill in."
        />
      </div>

      {/* Timeout */}
      <FormItem>
        <FormLabel>Timeout (ms)</FormLabel>
        <FormDescription>
          Optional timeout in milliseconds. If no signal is received within this
          duration, the step fails. Leave empty to wait indefinitely.
        </FormDescription>
        <FormControl>
          <MappingValueInput
            value={String(getValue('timeoutMs'))}
            onChange={(value) => updateField('timeoutMs', value)}
            valueType={getValueType('timeoutMs') as ValueMode}
            onValueTypeChange={(vt) => {
              // The runtime resolves timeoutMs and requires a numeric result:
              // template renders to a string and composite to an object, both
              // rejected at runtime. Restrict the mode cycle to
              // immediate ⇄ reference by skipping the unsupported modes.
              const next: ValueMode =
                vt === 'template'
                  ? 'reference'
                  : vt === 'composite'
                    ? 'immediate'
                    : vt;
              updateField('timeoutMs', getValue('timeoutMs'), next);
            }}
            fieldType="string"
            placeholder="e.g. 86400000 (24 hours)"
          />
        </FormControl>
      </FormItem>

      {/* Advanced Settings */}
      <div>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="px-0 text-xs text-muted-foreground"
          onClick={() => setShowAdvanced(!showAdvanced)}
        >
          {showAdvanced ? (
            <ChevronDown className="h-3 w-3 mr-1" />
          ) : (
            <ChevronRight className="h-3 w-3 mr-1" />
          )}
          Advanced Settings
        </Button>

        {showAdvanced && (
          <div className="mt-2 space-y-4">
            {/* Poll Interval */}
            <FormItem>
              <FormLabel>Poll Interval (ms)</FormLabel>
              <FormDescription>
                How often to check for the signal, as a whole number of
                milliseconds (default: 1000ms). Lower values mean faster
                response but more server load. Leave empty for the default.
              </FormDescription>
              <FormControl>
                <MappingValueInput
                  value={String(getValue('pollIntervalMs'))}
                  onChange={(value) => {
                    // Backend type is u64 — reject any non-integer input
                    // (decimals/signs/exponents) at the keystroke level.
                    const next = value ?? '';
                    if (/^\d*$/.test(next)) {
                      updateField('pollIntervalMs', next);
                    }
                  }}
                  valueType={getValueType('pollIntervalMs') as ValueMode}
                  onValueTypeChange={(vt) =>
                    updateField(
                      'pollIntervalMs',
                      getValue('pollIntervalMs'),
                      vt
                    )
                  }
                  fieldType="string"
                  placeholder="1000"
                  hideReferenceToggle
                />
              </FormControl>
            </FormItem>

            <FormItem>
              <FormLabel>Action Key</FormLabel>
              <FormDescription>
                Stable key for reports and action consumers.
              </FormDescription>
              <FormControl>
                <Input
                  value={String(getValue('actionKey'))}
                  onChange={(event) =>
                    updateField('actionKey', event.target.value, 'immediate')
                  }
                  placeholder="case_review_decision"
                />
              </FormControl>
            </FormItem>

            <FormItem>
              <FormLabel>Action Correlation</FormLabel>
              <FormDescription>
                DSL input-mapping object used for action correlation fields.
              </FormDescription>
              <MappingObjectField
                value={getValue('actionCorrelation')}
                onChange={(next) =>
                  updateField('actionCorrelation', next, 'composite')
                }
                jsonPlaceholder='{"caseId": {"valueType": "reference", "value": "data.caseId"}}'
              />
            </FormItem>

            <FormItem>
              <FormLabel>Action Context</FormLabel>
              <FormDescription>
                Optional non-authoritative display/query context.
              </FormDescription>
              <MappingObjectField
                value={getValue('actionContext')}
                onChange={(next) =>
                  updateField('actionContext', next, 'composite')
                }
                jsonPlaceholder='{"summary": {"valueType": "template", "value": "Case {{ data.caseId }}"}}'
              />
            </FormItem>

            <FormItem>
              <FormLabel>On-Wait Flow</FormLabel>
              {isOnWaitContainer ? (
                <FormDescription>
                  Runs before the workflow suspends. Edited visually — add,
                  connect and configure the steps inside this container on the
                  timeline or canvas. Inside the flow,{' '}
                  <code>variables._signal_id</code> holds the signal id
                  external systems must use to resume this step.
                </FormDescription>
              ) : showOnWaitJson ? (
                <>
                  <FormDescription>
                    Optional execution graph that runs before the workflow
                    suspends. This graph could not be opened as a visual
                    container (it has no steps); edit the JSON directly or
                    clear it.
                  </FormDescription>
                  <FormControl>
                    <Textarea
                      value={onWaitText}
                      onChange={(event) => updateOnWait(event.target.value)}
                      placeholder='{"steps": {}, "executionPlan": []}'
                      className="min-h-32 font-mono text-sm"
                    />
                  </FormControl>
                  {onWaitError && (
                    <p className="text-xs font-medium text-destructive">
                      {onWaitError}
                    </p>
                  )}
                </>
              ) : nodeId ? (
                <>
                  <FormDescription>
                    Optional flow that runs before the workflow suspends —
                    typically used to notify an external system of{' '}
                    <code>variables._signal_id</code>.
                  </FormDescription>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => convertNodeToContainer(nodeId)}
                  >
                    <ListTree className="mr-2 h-4 w-4" />
                    Add on-wait flow
                  </Button>
                </>
              ) : (
                <FormDescription>
                  Optional flow that runs before the workflow suspends. Create
                  this step first, then add the on-wait flow from the timeline
                  or canvas.
                </FormDescription>
              )}
            </FormItem>
          </div>
        )}
      </div>

      <div className="rounded-md border border-blue-500/50 bg-blue-500/10 p-3 text-sm">
        <p className="text-blue-600 dark:text-blue-400">
          Wire this step as an AI Agent tool to enable human-in-the-loop
          workflows. When the LLM calls this tool, execution suspends until a
          human provides input matching the response schema.
        </p>
      </div>
    </div>
  );
}
