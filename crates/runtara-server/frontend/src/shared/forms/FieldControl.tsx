import { Checkbox } from '@/shared/components/ui/checkbox';
import { FileInput } from '@/shared/components/ui/file-input';
import { Input } from '@/shared/components/ui/input';
import { KeyValueInput } from '@/shared/components/ui/key-value-input';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import { TagInput } from '@/shared/components/ui/tag-input';
import { Textarea } from '@/shared/components/ui/textarea';

import { inferControlKind, optionsFor } from './control-registry';
import type { FormField } from './types';

function optionKey(value: unknown): string {
  return JSON.stringify(value);
}

interface FieldControlProps {
  id: string;
  field: FormField;
  value: unknown;
  disabled: boolean;
  invalid?: boolean;
  onChange: (value: unknown) => void;
}

export function FieldControl({
  id,
  field,
  value,
  disabled,
  invalid,
  onChange,
}: FieldControlProps) {
  const kind = inferControlKind(field);
  const options = optionsFor(field);
  const common = {
    id,
    disabled,
    'aria-invalid': invalid || undefined,
  };

  if (kind === 'toggle') {
    return (
      <Checkbox
        {...common}
        checked={Boolean(value)}
        onCheckedChange={(checked) => onChange(checked === true)}
      />
    );
  }

  if (kind === 'textarea' || kind === 'secret_textarea') {
    return (
      <Textarea
        {...common}
        value={typeof value === 'string' ? value : ''}
        placeholder={field.placeholder}
        rows={6}
        onChange={(event) => onChange(event.target.value)}
        className={kind === 'secret_textarea' ? 'font-mono' : undefined}
      />
    );
  }

  if (kind === 'select' || (kind === 'lookup' && options.length > 0)) {
    return (
      <Select
        disabled={disabled}
        value={value === undefined || value === null ? '' : optionKey(value)}
        onValueChange={(next) =>
          onChange(
            options.find((option) => optionKey(option.value) === next)?.value
          )
        }
      >
        <SelectTrigger id={id} aria-invalid={invalid || undefined}>
          <SelectValue placeholder={field.placeholder ?? 'Select a value'} />
        </SelectTrigger>
        <SelectContent>
          {options.map((option) => (
            <SelectItem
              key={optionKey(option.value)}
              value={optionKey(option.value)}
            >
              {option.label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    );
  }

  if (kind === 'radio') {
    return (
      <div
        className="flex flex-wrap gap-3"
        role="radiogroup"
        aria-invalid={invalid}
      >
        {options.map((option) => {
          const checked = optionKey(option.value) === optionKey(value);
          return (
            <label
              key={optionKey(option.value)}
              className="flex items-center gap-2 text-sm"
            >
              <input
                type="radio"
                name={id}
                checked={checked}
                disabled={disabled}
                onChange={() => onChange(option.value)}
              />
              {option.label}
            </label>
          );
        })}
      </div>
    );
  }

  if (kind === 'multi_select') {
    const selected = Array.isArray(value) ? value.map(optionKey) : [];
    return (
      <select
        {...common}
        multiple
        value={selected}
        onChange={(event) =>
          onChange(
            Array.from(event.target.selectedOptions).map(
              (selectedOption) =>
                options.find(
                  (option) => optionKey(option.value) === selectedOption.value
                )?.value
            )
          )
        }
        className="min-h-24 w-full rounded-md border border-input bg-background px-3 py-2 text-sm"
      >
        {options.map((option) => (
          <option key={optionKey(option.value)} value={optionKey(option.value)}>
            {option.label}
          </option>
        ))}
      </select>
    );
  }

  if (kind === 'tags') {
    return (
      <TagInput
        id={id}
        disabled={disabled}
        value={Array.isArray(value) ? value.map(String) : []}
        placeholder={field.placeholder}
        onChange={onChange}
      />
    );
  }

  if (kind === 'key_value') {
    const entries =
      value && typeof value === 'object' && !Array.isArray(value)
        ? Object.fromEntries(
            Object.entries(value as Record<string, unknown>).map(
              ([key, item]) => [key, item == null ? '' : String(item)]
            )
          )
        : {};
    return (
      <KeyValueInput
        id={id}
        disabled={disabled}
        value={entries}
        onChange={onChange}
      />
    );
  }

  if (kind === 'file') {
    return (
      <FileInput
        disabled={disabled}
        value={value ? JSON.stringify(value) : ''}
        placeholder={field.placeholder}
        error={invalid ? 'Invalid file' : undefined}
        onChange={(next) => onChange(next ? JSON.parse(next) : undefined)}
      />
    );
  }

  if (kind === 'date_range' || kind === 'number_range') {
    const range = Array.isArray(value) ? value : ['', ''];
    const inputType = kind === 'date_range' ? 'date' : 'number';
    return (
      <div className="grid grid-cols-2 gap-2">
        {[0, 1].map((index) => (
          <Input
            key={index}
            {...common}
            id={`${id}-${index}`}
            type={inputType}
            value={String(range[index] ?? '')}
            onChange={(event) => {
              const next = [...range];
              next[index] =
                inputType === 'number' && event.target.value !== ''
                  ? Number(event.target.value)
                  : event.target.value;
              onChange(next);
            }}
          />
        ))}
      </div>
    );
  }

  const isNumber = kind === 'number';
  return (
    <Input
      {...common}
      type={
        kind === 'password'
          ? 'password'
          : kind === 'date'
            ? 'date'
            : kind === 'datetime'
              ? 'datetime-local'
              : isNumber
                ? 'number'
                : field.format === 'email' ||
                    field.format === 'url' ||
                    field.format === 'tel'
                  ? field.format
                  : 'text'
      }
      value={
        typeof value === 'string' || typeof value === 'number' ? value : ''
      }
      placeholder={field.placeholder}
      min={field.min}
      max={field.max}
      step={field.type === 'integer' ? 1 : undefined}
      onChange={(event) =>
        onChange(
          isNumber && event.target.value !== ''
            ? Number(event.target.value)
            : event.target.value
        )
      }
    />
  );
}
