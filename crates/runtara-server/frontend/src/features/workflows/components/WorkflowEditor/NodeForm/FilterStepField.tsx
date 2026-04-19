import { useContext, useEffect, useMemo, useRef } from 'react';
import { useFormContext, useWatch } from 'react-hook-form';
import { NodeFormContext } from './NodeFormContext';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { ConditionEditor } from '@/shared/components/ui/condition-editor';

type FilterStepFieldProps = {
  name: string;
};

export function FilterStepField({ name }: FilterStepFieldProps) {
  const form = useFormContext();
  const { previousSteps, nodeId } = useContext(NodeFormContext);
  const stepType = useWatch({ name: 'stepType', control: form.control });
  const filterCondition = useWatch({
    name: 'filterCondition',
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

  // Build array source suggestions from previous steps
  const arraySuggestions = useMemo(() => {
    const suggestions: { label: string; value: string }[] = [];

    previousSteps.forEach((step) => {
      step.outputs.forEach((output) => {
        if (output.type === 'array') {
          suggestions.push({
            label: `${step.name}${output.name ? ` → ${output.name}` : ''}`,
            value: output.path,
          });
        }
      });

      suggestions.push({
        label: `${step.name} → outputs`,
        value: `steps['${step.id}'].outputs`,
      });
      suggestions.push({
        label: `${step.name} → outputs.items`,
        value: `steps['${step.id}'].outputs.items`,
      });
    });

    suggestions.push({
      label: 'data.items (workflow input)',
      value: 'data.items',
    });

    return suggestions;
  }, [previousSteps]);

  // Initialize default condition for new Filter steps
  useEffect(() => {
    if (stepType !== 'Filter') return;
    if (nodeId) return; // Don't reset in edit mode
    if (hasInitializedRef.current) return;

    if (!filterCondition) {
      hasInitializedRef.current = true;
      form.setValue('filterCondition', defaultCondition);
    }
  }, [stepType, nodeId, filterCondition, form, defaultCondition]);

  if (stepType !== 'Filter') {
    return null;
  }

  const handleConditionChange = (value: string) => {
    try {
      const condition = JSON.parse(value);
      form.setValue('filterCondition', condition, { shouldDirty: true });
    } catch (e) {
      console.error('Failed to parse condition:', e);
    }
  };

  const conditionValue = filterCondition
    ? JSON.stringify(filterCondition)
    : JSON.stringify(defaultCondition);

  return (
    <div className="space-y-6">
      {/* Array Source Selection */}
      <div className="space-y-2">
        <Label className="text-sm font-medium">Array Source</Label>
        <p className="text-xs text-muted-foreground">
          Select the array to filter. Items matching the condition will be kept.
        </p>
        <Select
          value={form.getValues(`${name}.0.value`) || ''}
          onValueChange={(value) => {
            form.setValue(
              name,
              [
                {
                  type: 'value',
                  value,
                  typeHint: 'auto',
                  valueType: 'reference',
                },
              ],
              { shouldDirty: true }
            );
          }}
        >
          <SelectTrigger>
            <SelectValue placeholder="Select array source..." />
          </SelectTrigger>
          <SelectContent>
            {arraySuggestions.map((suggestion) => (
              <SelectItem key={suggestion.value} value={suggestion.value}>
                {suggestion.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <p className="text-xs text-muted-foreground">Or enter a custom path:</p>
        <Input
          placeholder="e.g., steps['fetch'].outputs.items"
          value={form.getValues(`${name}.0.value`) || ''}
          onChange={(e) => {
            form.setValue(
              name,
              [
                {
                  type: 'value',
                  value: e.target.value,
                  typeHint: 'auto',
                  valueType: 'reference',
                },
              ],
              { shouldDirty: true }
            );
          }}
        />
      </div>

      {/* Filter Condition */}
      <div className="space-y-2">
        <Label className="text-sm font-medium">Filter Condition</Label>
        <p className="text-xs text-muted-foreground">
          Items matching this condition will be kept. Use{' '}
          <code className="bg-muted px-1 rounded">item.*</code> to reference
          properties of each array element.
        </p>
        <ConditionEditor
          value={conditionValue}
          onChange={handleConditionChange}
          previousSteps={previousSteps}
        />
      </div>
    </div>
  );
}
