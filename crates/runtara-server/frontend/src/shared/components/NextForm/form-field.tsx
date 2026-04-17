import { useController, useFormContext, useWatch } from 'react-hook-form';
import { TextInput } from '../text-input.tsx';
import { SelectInput } from '../select-input.tsx';
import { PasswordField } from '../password-field';
import { CheckboxInput } from '../checkbox-input.tsx';
import { TextareaInput } from '../textarea-input.tsx';
import { TagInputField } from '../tag-input-field.tsx';

interface Props {
  className: string;
  name: string;
  type: string;
  label: string;
  options: any[];
  isDisabled: any;
  observedFields?: any;
  onChange?: (props: any) => {};
  description?: string;
  placeholder?: string;
}

export function FormField(props: Props) {
  const {
    className,
    name,
    type,
    label,
    options,
    isDisabled,
    observedFields,
    onChange,
    description,
    placeholder,
  } = props;

  const {
    field,
    // fieldState,
    formState,
  } = useController({ name });

  const formContext = useFormContext();

  const values = useWatch({ name: observedFields });

  const isFieldDisabled = Boolean(
    typeof isDisabled === 'function'
      ? isDisabled(values, options)
      : Boolean(isDisabled)
  );

  const handleChange = (event: any) => {
    // Handle both event objects and direct values
    if (event && event.target) {
      field.onChange(event);
    } else {
      field.onChange(event);
    }

    if (onChange) {
      const { getValues, setValue, trigger } = formContext;
      const values = getValues();
      onChange({ values, setValue, trigger, formState });
    }
  };

  if (type === 'hidden') {
    return <input type="hidden" {...field} value={field.value || ''} />;
  } else if (type === 'text') {
    return (
      <TextInput
        className={className}
        label={label}
        name={name}
        type="text"
        placeholder={placeholder}
        onChange={handleChange}
        description={description}
      />
    );
  } else if (type === 'number') {
    return (
      <TextInput
        className={className}
        label={label}
        name={name}
        type="number"
        placeholder={placeholder}
        onChange={handleChange}
        description={description}
      />
    );
  } else if (type === 'select') {
    const { name } = field;
    return (
      <SelectInput
        className={className}
        label={label}
        name={name}
        options={options}
        disabled={isFieldDisabled}
        onChange={handleChange}
        description={description}
      />
    );
  } else if (type === 'password') {
    return (
      <PasswordField
        className={className}
        label={label}
        name={name}
        placeholder={placeholder}
        onChange={handleChange}
        description={description}
      />
    );
  } else if (type === 'checkbox') {
    return (
      <CheckboxInput
        className={className}
        label={label}
        name={name}
        onChange={handleChange}
      />
    );
  } else if (type === 'textarea') {
    return (
      <TextareaInput
        className={className}
        label={label}
        name={name}
        placeholder={placeholder}
        onChange={handleChange}
        description={description}
        rows={6}
      />
    );
  } else if (type === 'tags') {
    return (
      <TagInputField
        name={name}
        label={label}
        placeholder={placeholder}
        onChange={handleChange}
        description={description}
      />
    );
  }
}
