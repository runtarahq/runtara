import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select.tsx';
import {
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
  FormDescription,
} from '@/shared/components/ui/form.tsx';

interface SelectOption {
  value: string;
  label: string;
  disabled?: boolean;
}

interface SelectInputProps {
  name: string;
  label?: string;
  options: SelectOption[];
  disabled?: boolean;
  onChange?: (value: string) => void;
  description?: string;
  className?: string;
  placeholder?: string;
}

export function SelectInput(props: SelectInputProps) {
  const { label, name, options, disabled, onChange, description, placeholder } =
    props;

  return (
    <FormField
      name={name}
      render={({ field }) => {
        return (
          <FormItem>
            {label && <FormLabel>{label}</FormLabel>}
            <Select
              onValueChange={onChange}
              value={field.value}
              disabled={disabled}
            >
              {/* FormControl is a Slot: it forwards the form item id and the
                  aria attributes to its immediate child. That child has to be
                  the trigger, not the Radix root, or they land on a component
                  that renders no DOM and the label above associates with
                  nothing. */}
              <FormControl>
                {/* The muted class lives here rather than on the shared
                    SelectTrigger so only this component's selects restyle. */}
                <SelectTrigger className="data-[placeholder]:text-muted-foreground">
                  <SelectValue placeholder={placeholder} />
                </SelectTrigger>
              </FormControl>
              <SelectContent>
                <SelectGroup>
                  {options.map((option) => (
                    <SelectItem
                      key={option.value}
                      value={option.value}
                      disabled={option.disabled}
                    >
                      {option.label}
                    </SelectItem>
                  ))}
                </SelectGroup>
              </SelectContent>
            </Select>
            {description && <FormDescription>{description}</FormDescription>}
            <FormMessage />
          </FormItem>
        );
      }}
    />
  );
}
