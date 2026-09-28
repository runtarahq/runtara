import { useContext, useEffect } from 'react';
import { useFormContext, useWatch } from 'react-hook-form';
import {
  FormControl,
  FormDescription,
  FormItem,
  FormLabel,
} from '@/shared/components/ui/form';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { NodeFormContext } from './NodeFormContext';
import {
  MappingValueInput,
  ValueMode,
} from './InputMappingField/MappingValueInput';
import { CompositeValueEditor } from './InputMappingField/CompositeValueEditor';
import type { CompositeArrayValue } from '@/features/workflows/stores/nodeFormStore';

type WaitForInstancesStepFieldProps = {
  name: string;
};

/**
 * `instanceIds` must resolve to an array of ids: a reference to an id array,
 * a literal JSON array, or a composite array of references (typically one
 * `steps.<start>.outputs.instanceId` per control start step). A template
 * renders to a string, so it is not offered.
 */
const INSTANCE_IDS_MODES: readonly ValueMode[] = [
  'reference',
  'immediate',
  'composite',
];

/** The composite array carried into composite mode from the current value. */
function compositeArray(value: unknown): CompositeArrayValue {
  if (Array.isArray(value)) return value as CompositeArrayValue;
  if (typeof value === 'string' && value.trim()) {
    try {
      const parsed = JSON.parse(value);
      if (Array.isArray(parsed)) {
        return parsed.map((item) => ({
          valueType: 'immediate',
          value: item,
        })) as CompositeArrayValue;
      }
    } catch {
      // Not a JSON array: start from an empty list.
    }
  }
  return [];
}

/** `timeoutMs` must resolve to a positive integer. */
const TIMEOUT_MODES: readonly ValueMode[] = ['immediate', 'reference'];

const TYPE_HINTS: Record<string, string> = {
  instanceIds: 'array',
  mode: 'string',
  timeoutMs: 'number',
};

/** Initial inputMapping rows for a new WaitForInstances step. */
function defaultWaitForInstancesMapping() {
  return [
    {
      type: 'instanceIds',
      value: '',
      typeHint: 'array',
      valueType: 'reference',
    },
    { type: 'mode', value: 'all', typeHint: 'string', valueType: 'immediate' },
    {
      type: 'timeoutMs',
      value: '',
      typeHint: 'number',
      valueType: 'immediate',
    },
  ];
}

export function WaitForInstancesStepField({
  name,
}: WaitForInstancesStepFieldProps) {
  const form = useFormContext();
  const { nodeId } = useContext(NodeFormContext);
  const stepType = useWatch({ name: 'stepType', control: form.control });

  useEffect(() => {
    if (stepType !== 'WaitForInstances') return;
    if (nodeId) return; // Don't reset in edit mode

    const currentMapping = form.getValues(name) || [];
    if (currentMapping.length === 0) {
      form.setValue(name, defaultWaitForInstancesMapping());
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [stepType, nodeId]);

  const inputMapping = useWatch({
    name,
    control: form.control,
    defaultValue: [],
  });

  if (stepType !== 'WaitForInstances') {
    return null;
  }

  const findField = (fieldName: string) =>
    (inputMapping || []).find((item: any) => item.type === fieldName);

  const getValue = (fieldName: string) => findField(fieldName)?.value ?? '';

  const getValueType = (fieldName: string, fallback: ValueMode) =>
    (findField(fieldName)?.valueType as ValueMode | undefined) || fallback;

  const updateField = (
    fieldName: string,
    value: any,
    valueType?: ValueMode
  ) => {
    const mapping = form.getValues(name) || [];
    const fieldIndex = mapping.findIndex(
      (item: any) => item.type === fieldName
    );
    const options = {
      shouldDirty: true,
      shouldTouch: true,
      shouldValidate: true,
    };

    if (fieldIndex >= 0) {
      form.setValue(`${name}.${fieldIndex}.value`, value, options);
      if (valueType !== undefined) {
        form.setValue(`${name}.${fieldIndex}.valueType`, valueType, options);
      }
      return;
    }

    form.setValue(
      name,
      [
        ...mapping,
        {
          type: fieldName,
          value,
          typeHint: TYPE_HINTS[fieldName] ?? 'string',
          valueType: valueType || 'immediate',
        },
      ],
      options
    );
  };

  const idsValueType = getValueType('instanceIds', 'reference');

  return (
    <div className="space-y-4">
      <div>
        <p className="text-sm font-medium">Wait for Instances Configuration</p>
        <p className="text-xs text-muted-foreground">
          Parks the run, without holding a runner, until child runs it started
          have finished. The workflow must be durable.
        </p>
      </div>

      <FormItem>
        <FormLabel>Instance IDs *</FormLabel>
        <FormDescription>
          Array of 1 to 1000 distinct instance ids, each a direct child of this
          run — a reference to an id array, or a composite list of each control
          start step's <code>outputs.instanceId</code>.
        </FormDescription>
        <FormControl>
          <MappingValueInput
            value={
              idsValueType === 'composite'
                ? ''
                : String(getValue('instanceIds'))
            }
            onChange={(value) => updateField('instanceIds', value ?? '')}
            valueType={idsValueType}
            onValueTypeChange={(vt) =>
              updateField(
                'instanceIds',
                vt === 'composite'
                  ? compositeArray(getValue('instanceIds'))
                  : idsValueType === 'composite'
                    ? ''
                    : getValue('instanceIds'),
                vt
              )
            }
            modes={INSTANCE_IDS_MODES}
            fieldType="array"
            fieldName="instanceIds"
            placeholder='["instance-id-1", "instance-id-2"]'
          />
        </FormControl>
        {idsValueType === 'composite' && (
          <div className="overflow-hidden rounded-md border bg-muted/20">
            <CompositeValueEditor
              value={compositeArray(getValue('instanceIds'))}
              onChange={(value) =>
                updateField('instanceIds', value, 'composite')
              }
              showCloseButton={false}
            />
          </div>
        )}
      </FormItem>

      <FormItem>
        <FormLabel>Mode</FormLabel>
        <FormDescription>
          All settles when every run has finished; Any settles when the first
          has.
        </FormDescription>
        <Select
          value={getValue('mode') === 'any' ? 'any' : 'all'}
          onValueChange={(value) => updateField('mode', value, 'immediate')}
        >
          <FormControl>
            <SelectTrigger aria-label="Mode">
              <SelectValue placeholder="Select mode" />
            </SelectTrigger>
          </FormControl>
          <SelectContent>
            <SelectItem value="all">All</SelectItem>
            <SelectItem value="any">Any</SelectItem>
          </SelectContent>
        </Select>
      </FormItem>

      <FormItem>
        <FormLabel>Timeout (ms)</FormLabel>
        <FormDescription>
          Optional deadline from the first time the step runs. When it passes,
          the step settles with resolution <code>deadline</code> and what has
          finished so far; children keep running. Leave empty to wait
          indefinitely.
        </FormDescription>
        <FormControl>
          <MappingValueInput
            value={String(getValue('timeoutMs'))}
            onChange={(value) => updateField('timeoutMs', value ?? '')}
            valueType={getValueType('timeoutMs', 'immediate')}
            onValueTypeChange={(vt) =>
              updateField('timeoutMs', getValue('timeoutMs'), vt)
            }
            modes={TIMEOUT_MODES}
            fieldType="number"
            fieldName="timeoutMs"
            placeholder="e.g. 86400000 (24 hours)"
          />
        </FormControl>
      </FormItem>
    </div>
  );
}
