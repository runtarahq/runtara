import { useMemo } from 'react';
import { useFormContext, useWatch } from 'react-hook-form';
import { Label } from '@/shared/components/ui/label';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/shared/components/ui/select';
import {
  MappingValueInput,
  ValueMode,
} from './InputMappingField/MappingValueInput';
import { CompositeValueEditor } from './InputMappingField/CompositeValueEditor';
import type {
  CompositeArrayValue,
  CompositeObjectValue,
} from '@/features/workflows/stores/nodeFormStore';

type SourceSuggestion = {
  label: string;
  value: string;
};

type SourceMappingValueFieldProps = {
  name: string;
  label: string;
  description: string;
  suggestions: SourceSuggestion[];
  placeholder?: string;
};

type SourceEntry = {
  type: string;
  value?: unknown;
  typeHint?: string;
  valueType?: ValueMode;
  defaultValue?: unknown;
};

function getSourceEntry(mapping: unknown): SourceEntry {
  if (!Array.isArray(mapping)) {
    return {
      type: 'value',
      value: '',
      typeHint: 'auto',
      valueType: 'reference',
    };
  }

  const valueEntry = mapping.find(
    (item) =>
      typeof item === 'object' &&
      item !== null &&
      (item as { type?: unknown }).type === 'value'
  );

  if (!valueEntry || typeof valueEntry !== 'object') {
    return {
      type: 'value',
      value: '',
      typeHint: 'auto',
      valueType: 'reference',
    };
  }

  return valueEntry as SourceEntry;
}

function mappingInputValue(value: unknown): string | number | boolean | null {
  if (
    typeof value === 'string' ||
    typeof value === 'number' ||
    typeof value === 'boolean' ||
    value === null
  ) {
    return value;
  }
  if (value === undefined) return '';
  return JSON.stringify(value);
}

function compositeValue(
  value: unknown
): CompositeObjectValue | CompositeArrayValue {
  if (Array.isArray(value)) return value as CompositeArrayValue;
  if (value && typeof value === 'object') return value as CompositeObjectValue;
  if (typeof value === 'string' && value.trim()) {
    try {
      const parsed = JSON.parse(value);
      if (Array.isArray(parsed)) return parsed as CompositeArrayValue;
      if (parsed && typeof parsed === 'object') {
        return parsed as CompositeObjectValue;
      }
    } catch {
      // Fall through to an empty array source.
    }
  }
  return [];
}

export function SourceMappingValueField({
  name,
  label,
  description,
  suggestions,
  placeholder = 'Select or enter array source...',
}: SourceMappingValueFieldProps) {
  const form = useFormContext();
  const mapping = useWatch({ name, control: form.control });
  const sourceEntry = getSourceEntry(mapping);
  const valueType = sourceEntry.valueType || 'reference';
  const selectedReference =
    valueType === 'reference' && typeof sourceEntry.value === 'string'
      ? sourceEntry.value
      : '';
  const selectedSuggestion = useMemo(
    () =>
      suggestions.find((suggestion) => suggestion.value === selectedReference),
    [selectedReference, suggestions]
  );

  const setSourceEntry = (updates: Partial<SourceEntry>) => {
    const currentEntry = getSourceEntry(form.getValues(name));
    const hasUpdate = (field: keyof SourceEntry) =>
      Object.prototype.hasOwnProperty.call(updates, field);

    form.setValue(
      name,
      [
        {
          type: 'value',
          value: hasUpdate('value')
            ? updates.value
            : (currentEntry.value ?? ''),
          typeHint: hasUpdate('typeHint')
            ? updates.typeHint
            : (currentEntry.typeHint ?? 'auto'),
          valueType: hasUpdate('valueType')
            ? updates.valueType
            : (currentEntry.valueType ?? 'reference'),
          ...(hasUpdate('defaultValue')
            ? { defaultValue: updates.defaultValue }
            : currentEntry.defaultValue !== undefined
              ? { defaultValue: currentEntry.defaultValue }
              : {}),
        },
      ],
      { shouldDirty: true, shouldTouch: true, shouldValidate: true }
    );
  };

  return (
    <div className="space-y-2">
      <Label className="text-sm font-medium">{label}</Label>
      <p className="text-xs text-muted-foreground">{description}</p>

      {suggestions.length > 0 && (
        <Select
          value={selectedSuggestion?.value ?? ''}
          onValueChange={(value) =>
            setSourceEntry({
              value,
              valueType: 'reference',
              typeHint: 'auto',
            })
          }
        >
          <SelectTrigger>
            <SelectValue placeholder="Choose a known array output..." />
          </SelectTrigger>
          <SelectContent>
            {suggestions.map((suggestion) => (
              <SelectItem key={suggestion.value} value={suggestion.value}>
                {suggestion.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}

      <MappingValueInput
        value={
          valueType === 'composite' ? '' : mappingInputValue(sourceEntry.value)
        }
        onChange={(value) => setSourceEntry({ value })}
        valueType={valueType}
        onValueTypeChange={(nextValueType) =>
          setSourceEntry({
            valueType: nextValueType,
            value:
              nextValueType === 'composite'
                ? compositeValue(sourceEntry.value)
                : '',
          })
        }
        fieldType="array"
        fieldName="source"
        placeholder={placeholder}
        allowNull
        defaultValue={sourceEntry.defaultValue}
        onDefaultValueChange={(nextDefault) =>
          setSourceEntry({ defaultValue: nextDefault })
        }
      />

      {valueType === 'composite' && (
        <div className="overflow-hidden rounded-md border bg-muted/20">
          <CompositeValueEditor
            value={compositeValue(sourceEntry.value)}
            onChange={(value) =>
              setSourceEntry({ value, valueType: 'composite' })
            }
            showCloseButton={false}
          />
        </div>
      )}
    </div>
  );
}
