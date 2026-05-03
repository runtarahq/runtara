import { useContext, useEffect, useMemo, useRef } from 'react';
import { useFormContext, useWatch } from 'react-hook-form';
import { NodeFormContext } from './NodeFormContext';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import { ConditionEditor } from '@/shared/components/ui/condition-editor';

type WhileStepFieldProps = {
  name: string;
};

export function WhileStepField({ name }: WhileStepFieldProps) {
  const form = useFormContext();
  const { previousSteps } = useContext(NodeFormContext);
  const stepType = useWatch({ name: 'stepType', control: form.control });
  const whileCondition = useWatch({
    name: 'whileCondition',
    control: form.control,
  });
  const whileMaxIterations = useWatch({
    name: 'whileMaxIterations',
    control: form.control,
  });
  const whileTimeout = useWatch({
    name: 'whileTimeout',
    control: form.control,
  });

  const hasInitializedRef = useRef(false);

  const defaultCondition = useMemo(
    () => ({
      type: 'operation',
      op: 'EQ',
      arguments: [
        { valueType: 'immediate', value: '', immediateType: 'string' },
        { valueType: 'immediate', value: '', immediateType: 'string' },
      ],
    }),
    []
  );

  // Initialize missing defaults for While steps.
  // Timeline creation already has a node id, so missing-field checks are safer than nodeId.
  useEffect(() => {
    if (stepType !== 'While') return;
    if (hasInitializedRef.current) return;

    hasInitializedRef.current = true;

    if (!whileCondition) {
      form.setValue('whileCondition', defaultCondition);
    }
    if (whileMaxIterations === undefined || whileMaxIterations === null) {
      form.setValue('whileMaxIterations', 10);
    }
  }, [stepType, whileCondition, whileMaxIterations, form, defaultCondition]);

  if (stepType !== 'While') {
    return null;
  }

  const handleConditionChange = (value: string) => {
    try {
      const condition = JSON.parse(value);
      form.setValue('whileCondition', condition, { shouldDirty: true });
    } catch (e) {
      console.error('Failed to parse condition:', e);
    }
  };

  const conditionValue = whileCondition
    ? JSON.stringify(whileCondition)
    : JSON.stringify(defaultCondition);

  return (
    <div className="space-y-6" data-field-name={name}>
      {/* Loop Condition */}
      <div className="space-y-2">
        <Label className="text-sm font-medium">Loop Condition</Label>
        <p className="text-xs text-muted-foreground">
          The loop repeats while this condition is true. Use{' '}
          <code className="bg-muted px-1 rounded">loop.index</code> for the
          current iteration or{' '}
          <code className="bg-muted px-1 rounded">loop.outputs</code> for the
          previous iteration&apos;s Finish outputs.
        </p>
        <ConditionEditor
          value={conditionValue}
          onChange={handleConditionChange}
          previousSteps={previousSteps}
          isInsideWhileLoop={true}
        />
      </div>

      {/* Max Iterations */}
      <div className="space-y-2">
        <Label className="text-sm font-medium">Max Iterations</Label>
        <p className="text-xs text-muted-foreground">
          Safety limit to prevent infinite loops. Set to 0 for unlimited.
        </p>
        <Input
          type="number"
          min={0}
          value={whileMaxIterations ?? 10}
          onChange={(e) => {
            const val = parseInt(e.target.value, 10);
            form.setValue('whileMaxIterations', isNaN(val) ? 10 : val, {
              shouldDirty: true,
            });
          }}
          className="w-32"
        />
      </div>

      {/* Timeout */}
      <div className="space-y-2">
        <Label className="text-sm font-medium">Timeout (seconds)</Label>
        <p className="text-xs text-muted-foreground">
          Overall timeout for the entire While step. Leave empty for no timeout.
        </p>
        <Input
          type="number"
          min={0}
          placeholder="No timeout"
          value={whileTimeout ?? ''}
          onChange={(e) => {
            const raw = e.target.value;
            if (raw === '') {
              form.setValue('whileTimeout', null, { shouldDirty: true });
            } else {
              const val = parseInt(raw, 10);
              form.setValue('whileTimeout', isNaN(val) ? null : val, {
                shouldDirty: true,
              });
            }
          }}
          className="w-32"
        />
      </div>
    </div>
  );
}
