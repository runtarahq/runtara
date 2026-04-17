import {
  FormControl,
  FormField,
  FormItem,
  FormLabel,
} from '@/shared/components/ui/form.tsx';
import { Switch } from '@/shared/components/ui/switch.tsx';

interface CheckboxInputProps {
  name: string;
  label: string;
  disabled?: boolean;
  className?: string;
  onChange?: (event: unknown) => void;
}

export function CheckboxInput(props: CheckboxInputProps) {
  const { label, name, disabled } = props;

  return (
    <FormField
      name={name}
      render={({ field }) => (
        <FormItem className="flex flex-row items-center space-x-3 space-y-0">
          <FormControl>
            <Switch
              checked={field.value}
              disabled={disabled}
              onCheckedChange={field.onChange}
            />
          </FormControl>
          <FormLabel>{label}</FormLabel>
        </FormItem>
      )}
    />
  );
}
