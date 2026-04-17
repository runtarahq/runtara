import { ChangeEvent } from 'react';
import {
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
  FormDescription,
} from '@/shared/components/ui/form.tsx';
import { Input } from '@/shared/components/ui/input.tsx';

interface TextInputProps {
  name: string;
  label?: string;
  className?: string;
  type?: 'text' | 'email' | 'password' | 'number' | 'tel' | 'url';
  placeholder?: string;
  showError?: boolean;
  onChange?: (e: ChangeEvent<HTMLInputElement>) => void;
  description?: string;
}

export function TextInput(props: TextInputProps) {
  const {
    className,
    name,
    label,
    type = 'text',
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
              <Input
                className={className}
                {...field}
                type={type}
                placeholder={placeholder}
                onChange={onChange}
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
