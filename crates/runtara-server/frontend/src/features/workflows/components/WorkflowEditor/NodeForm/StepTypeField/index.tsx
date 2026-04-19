import { useMemo } from 'react';
import { FieldValues, useFormContext } from 'react-hook-form';
import {
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
  FormDescription,
} from '@/shared/components/ui/form.tsx';
import { StepTypeIcon } from '@/features/workflows/components/StepTypeIcon';
import { StepTypeInfo } from '@/generated/RuntaraRuntimeApi.ts';
import { cn } from '@/lib/utils';

export function StepTypeField(props: any) {
  const { label, name, description } = props;

  const { stepTypes, setValue }: FieldValues = useFormContext();

  const options = useMemo(
    () =>
      stepTypes.map((stepType: StepTypeInfo) => ({
        ...stepType,
        label: stepType.name,
        value: stepType.name,
      })),
    [stepTypes]
  );

  const onChange = (value: string) => {
    if (value !== 'Agent') {
      setValue('agentId', '');
      setValue('capabilityId', '');
    }
  };

  return (
    <FormField
      name={name}
      render={({ field }) => {
        return (
          <FormItem>
            <FormLabel>{label}</FormLabel>
            {description && <FormDescription>{description}</FormDescription>}
            <FormControl>
              <div className="grid grid-cols-2 gap-2">
                {options.map((option: any) => {
                  const isSelected = field.value === option.value;
                  return (
                    <button
                      key={option.value}
                      type="button"
                      onClick={() => {
                        field.onChange(option.value);
                        onChange(option.value);
                      }}
                      className={cn(
                        'flex flex-col items-center gap-2 p-3 rounded-lg border-2 transition-all hover:border-primary/50',
                        isSelected
                          ? 'border-primary bg-primary/5'
                          : 'border-border bg-background'
                      )}
                    >
                      <StepTypeIcon
                        className={cn(
                          'w-6 h-6 transition-colors',
                          isSelected ? 'text-primary' : 'text-muted-foreground'
                        )}
                        type={option.value}
                      />
                      <div className="text-center">
                        <div
                          className={cn(
                            'text-sm font-medium',
                            isSelected ? 'text-primary' : 'text-foreground'
                          )}
                        >
                          {option.label}
                        </div>
                        {option.description && (
                          <div className="text-[0.7rem] text-muted-foreground mt-0.5 leading-tight">
                            {option.description}
                          </div>
                        )}
                      </div>
                    </button>
                  );
                })}
              </div>
            </FormControl>
            <FormMessage />
          </FormItem>
        );
      }}
    />
  );
}
