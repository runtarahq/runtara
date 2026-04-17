import {
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
  FormDescription,
} from '@/shared/components/ui/form.tsx';
import { TagInput } from '@/shared/components/ui/tag-input.tsx';

interface TagInputFieldProps {
  name: string;
  label?: string;
  className?: string;
  placeholder?: string;
  showError?: boolean;
  onChange?: (value: string[]) => void;
  description?: string;
}

export function TagInputField(props: TagInputFieldProps) {
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
        return (
          <FormItem>
            {label && <FormLabel>{label}</FormLabel>}
            <FormControl>
              <TagInput
                value={Array.isArray(field.value) ? field.value : []}
                onChange={(val) => {
                  field.onChange(val);
                  onChange?.(val);
                }}
                placeholder={placeholder}
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
