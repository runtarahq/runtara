import { useState } from 'react';
import { Eye, EyeOff } from 'lucide-react';
import {
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
  FormDescription,
} from '@/shared/components/ui/form.tsx';
import { Input } from '@/shared/components/ui/input.tsx';
import { Button } from '@/shared/components/ui/button.tsx';

type Props = {
  className?: string;
  label?: string;
  name: string;
  placeholder?: string;
  onChange: (event: React.ChangeEvent<HTMLInputElement>) => void;
  description?: string;
};

export function PasswordField(props: Props) {
  const { className, label, name, placeholder, description } = props;

  const [isVisible, setIsVisible] = useState<boolean>(false);

  const toggle = () => setIsVisible((prev) => !prev);

  const type = isVisible ? 'text' : 'password';

  const Icon = isVisible ? EyeOff : Eye;

  return (
    <FormField
      name={name}
      render={({ field }) => {
        return (
          <FormItem>
            {label && <FormLabel>{label}</FormLabel>}
            <FormControl>
              <div className="relative">
                <Input
                  className={className}
                  {...field}
                  type={type}
                  placeholder={placeholder}
                />
                <Button
                  className="absolute inset-y-0 end-0 flex h-full w-9 items-center justify-center rounded-e-lg border border-transparent text-muted-foreground/80 outline-offset-2 transition-colors hover:text-foreground focus-visible:outline focus-visible:outline-2 focus-visible:outline-ring/70 disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50"
                  type="button"
                  size="icon"
                  variant="link"
                  onClick={toggle}
                >
                  <Icon className="w-4 h-4" />
                </Button>
              </div>
            </FormControl>
            {description && <FormDescription>{description}</FormDescription>}
            <FormMessage />
          </FormItem>
        );
      }}
    />
  );
}
