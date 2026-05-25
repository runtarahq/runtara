import {
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
  FormDescription,
} from '@/shared/components/ui/form.tsx';
import { KeyValueInput } from '@/shared/components/ui/key-value-input.tsx';

interface KeyValueInputFieldProps {
  name: string;
  label?: string;
  className?: string;
  placeholder?: string;
  showError?: boolean;
  onChange?: (value: Record<string, string>) => void;
  description?: string;
}

/**
 * react-hook-form binding for `KeyValueInput`. Mirrors the `TagInputField`
 * pattern. Used by the DynamicConnectionForm for `HashMap<String, String>`
 * connection params (e.g. extra headers, tool hints).
 */
export function KeyValueInputField(props: KeyValueInputFieldProps) {
  const {
    name,
    label,
    placeholder,
    showError = true,
    onChange,
    description,
  } = props;

  return (
    <FormField
      name={name}
      render={({ field }) => {
        const value =
          field.value && typeof field.value === 'object' && !Array.isArray(field.value)
            ? (field.value as Record<string, string>)
            : {};
        return (
          <FormItem>
            {label && <FormLabel>{label}</FormLabel>}
            <FormControl>
              <KeyValueInput
                value={value}
                onChange={(val) => {
                  field.onChange(val);
                  onChange?.(val);
                }}
                valuePlaceholder={placeholder}
              />
            </FormControl>
            {description && <FormDescription>{description}</FormDescription>}
            {showError && <FormMessage />}
          </FormItem>
        );
      }}
    />
  );
}
