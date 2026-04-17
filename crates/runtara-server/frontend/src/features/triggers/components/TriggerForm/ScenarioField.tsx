import { useMemo } from 'react';
import { useController } from 'react-hook-form';
import { ScenarioDto } from '@/generated/RuntaraRuntimeApi';
import { SelectInput } from '@/shared/components/select-input.tsx';

export function ScenarioField(props: any) {
  const { label, name, disabled, scenarios } = props;

  const { field } = useController({ name });

  const options = useMemo(
    () =>
      scenarios.map((scenario: ScenarioDto) => ({
        ...scenario,
        label: scenario.name,
        value: scenario.id,
      })),
    [scenarios]
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
