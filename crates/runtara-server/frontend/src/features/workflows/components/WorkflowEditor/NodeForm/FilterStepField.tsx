import { useContext, useEffect, useMemo, useRef } from 'react';
import { useFormContext, useWatch } from 'react-hook-form';
import { NodeFormContext } from './NodeFormContext';
import { Label } from '@/shared/components/ui/label';
import { ConditionEditor } from '@/shared/components/ui/condition-editor';
import { SourceMappingValueField } from './SourceMappingValueField';

type FilterStepFieldProps = {
  name: string;
};

export function FilterStepField({ name }: FilterStepFieldProps) {
  const form = useFormContext();
  const { previousSteps, nodeId, inputSchemaFields, variables } =
    useContext(NodeFormContext);
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
      <SourceMappingValueField
        name={name}
        label="Array Source"
        description="Select the array to filter. Items matching the condition will be kept."
        suggestions={arraySuggestions}
        placeholder="e.g., steps['fetch'].outputs.items"
      />

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
          inputSchemaFields={inputSchemaFields}
          variables={variables}
        />
      </div>
    </div>
  );
}
