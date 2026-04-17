import { ChangeEvent } from 'react';
import {
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
  FormDescription,
} from '@/shared/components/ui/form.tsx';
import { Textarea } from '@/shared/components/ui/textarea.tsx';

interface TextareaInputProps {
  name: string;
  label?: string;
  className?: string;
  placeholder?: string;
  showError?: boolean;
  onChange?: (e: ChangeEvent<HTMLTextAreaElement>) => void;
  description?: string;
  rows?: number;
}

export function TextareaInput(props: TextareaInputProps) {
  const {
    className,
    name,
    label,
    placeholder,
    showError = true,
    onChange,
    description,
    rows = 4,
  } = props;

  return (
    <FormField
      name={name}
      render={({ field }) => {
        return (
          <FormItem>
            {label && <FormLabel>{label}</FormLabel>}
            <FormControl>
              <Textarea
                className={className}
                {...field}
                placeholder={placeholder}
                onChange={onChange ?? field.onChange}
                rows={rows}
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
