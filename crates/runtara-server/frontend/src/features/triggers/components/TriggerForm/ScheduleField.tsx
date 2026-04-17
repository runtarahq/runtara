import { useController, useWatch } from 'react-hook-form';
import { FormLabel } from '@/shared/components/ui/form';
import { ScheduleBuilder } from './ScheduleBuilder';
import { type ScheduleConfig } from '@/features/triggers/utils/cron';

interface ScheduleFieldProps {
  label: string;
  disabled?: boolean;
}

const defaultConfig: ScheduleConfig = {
  type: 'interval',
  intervalValue: 5,
  intervalUnit: 'minutes',
};

export function ScheduleField({ label, disabled }: ScheduleFieldProps) {
  const { field } = useController({
    name: 'scheduleConfig',
  });

  const triggerTypeWatch = useWatch({ name: 'triggerType' });

  if (triggerTypeWatch !== 'CRON') {
    return null;
  }

  // Ensure we always have a valid ScheduleConfig
  const scheduleValue: ScheduleConfig =
    field.value && typeof field.value === 'object' && 'type' in field.value
      ? (field.value as ScheduleConfig)
      : defaultConfig;

  return (
    <div className="space-y-3">
      <FormLabel>{label}</FormLabel>
      <ScheduleBuilder
        value={scheduleValue}
        onChange={field.onChange}
        disabled={disabled}
      />
    </div>
  );
}
