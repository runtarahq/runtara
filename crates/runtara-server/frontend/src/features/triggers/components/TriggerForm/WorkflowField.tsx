import { useMemo } from 'react';
import { useController } from 'react-hook-form';
import { WorkflowDto } from '@/generated/RuntaraRuntimeApi';
import { SelectInput } from '@/shared/components/select-input.tsx';

export function WorkflowField(props: any) {
  const { label, name, disabled, workflows } = props;

  const { field } = useController({ name });

  const options = useMemo(
    () =>
      workflows.map((workflow: WorkflowDto) => ({
        ...workflow,
        label: workflow.name,
        value: workflow.id,
      })),
    [workflows]
  );

  const handleChange = (value: string) => field.onChange(value);

  return (
    <SelectInput
      label={label}
      name={name}
      options={options}
      disabled={disabled}
      onChange={handleChange}
    />
  );
}
