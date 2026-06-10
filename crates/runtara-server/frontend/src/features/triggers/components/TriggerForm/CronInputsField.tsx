import { useController, useWatch } from 'react-hook-form';
import { FormLabel } from '@/shared/components/ui/form';
import { Textarea } from '@/shared/components/ui/textarea';
import { staticInputsError } from '@/features/triggers/utils/trigger-configuration';

interface CronInputsFieldProps {
  label: string;
  disabled?: boolean;
}

/**
 * Optional JSON textarea for CRON triggers that round-trips
 * `configuration.inputs` (the static input envelope sent on each fire).
 */
export function CronInputsField({ label, disabled }: CronInputsFieldProps) {
  const { field, fieldState } = useController({ name: 'cronInputs' });
  const triggerTypeWatch = useWatch({ name: 'triggerType' });

  if (triggerTypeWatch !== 'CRON') {
    return null;
  }

  const value = typeof field.value === 'string' ? field.value : '';
  // Validate as the user types; the form schema also blocks save while invalid.
  const error = staticInputsError(value) ?? fieldState.error?.message ?? null;

  return (
    <div className="space-y-2">
      <FormLabel>{label}</FormLabel>
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
      {error && (
        <p className="text-[0.8rem] font-medium text-destructive">{error}</p>
      )}
    </div>
  );
}
