import { useContext, useMemo } from 'react';
import { useFormContext, useWatch } from 'react-hook-form';
import { NodeFormContext } from './NodeFormContext';
import { Input } from '@/shared/components/ui/input';
import { Label } from '@/shared/components/ui/label';
import { TagInput } from '@/shared/components/ui/tag-input';
import { SourceMappingValueField } from './SourceMappingValueField';

type GroupByStepFieldProps = {
  name: string;
};

export function GroupByStepField({ name }: GroupByStepFieldProps) {
  const form = useFormContext();
  const { previousSteps } = useContext(NodeFormContext);
  const stepType = useWatch({ name: 'stepType', control: form.control });
  const groupByKey = useWatch({ name: 'groupByKey', control: form.control });
  const groupByExpectedKeys = useWatch({
    name: 'groupByExpectedKeys',
    control: form.control,
  });

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

  if (stepType !== 'GroupBy') {
    return null;
  }

  return (
    <div className="space-y-6">
      <SourceMappingValueField
        name={name}
        label="Array Source"
        description="Select the array to group. Items will be grouped by the specified key."
        suggestions={arraySuggestions}
        placeholder="e.g., steps['fetch'].outputs.items"
      />

      {/* Group Key */}
      <div className="space-y-2">
        <Label className="text-sm font-medium">Group Key</Label>
        <p className="text-xs text-muted-foreground">
          Property path to group by. Use dot notation for nested properties
          (e.g., <code className="bg-muted px-1 rounded">status</code>,{' '}
          <code className="bg-muted px-1 rounded">user.role</code>,{' '}
          <code className="bg-muted px-1 rounded">data.category</code>).
        </p>
        <Input
          placeholder="e.g., status"
          value={groupByKey || ''}
          onChange={(e) => {
            form.setValue('groupByKey', e.target.value, { shouldDirty: true });
          }}
        />
      </div>

      {/* Expected Keys (optional) */}
      <div className="space-y-2">
        <Label className="text-sm font-medium">
          Expected Keys{' '}
          <span className="text-muted-foreground font-normal">(optional)</span>
        </Label>
        <p className="text-xs text-muted-foreground">
          Pre-define expected key values. These keys will always appear in
          output with count=0 if no items match. Type a value and press Enter to
          add.
        </p>
        <TagInput
          value={groupByExpectedKeys || []}
          onChange={(value) => {
            form.setValue('groupByExpectedKeys', value, {
              shouldDirty: true,
            });
          }}
          placeholder="Type and press Enter to add..."
        />
      </div>
    </div>
  );
}
