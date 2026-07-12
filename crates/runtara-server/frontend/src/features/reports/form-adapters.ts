import type { FormField, FormOption } from '@/shared/forms';

import type { ReportEditorConfig, ReportFilterDefinition } from './types';

type FilterOption = { value: unknown; label: string; count?: number };

export function reportFilterToFormField(
  filter: ReportFilterDefinition,
  options: FilterOption[] = []
): FormField {
  const formOptions: FormOption[] = options.map((option) => ({
    value: option.value,
    label:
      option.count === undefined
        ? option.label
        : `${option.label} (${option.count})`,
  }));
  switch (filter.type) {
    case 'multi_select':
      return {
        type: 'array',
        label: filter.label,
        control: { kind: 'multi_select', options: formOptions },
      };
    case 'radio':
      return {
        type: 'string',
        label: filter.label,
        control: { kind: 'radio', options: formOptions },
      };
    case 'select':
      return {
        type: 'string',
        label: filter.label,
        control: { kind: 'select', options: formOptions },
      };
    case 'checkbox':
      return {
        type: 'boolean',
        label: filter.label,
        control: { kind: 'toggle' },
      };
    case 'number_range':
      return {
        type: 'array',
        label: filter.label,
        control: { kind: 'number_range' },
      };
    case 'time_range':
      return {
        type: 'string',
        label: filter.label,
        control: { kind: 'select', options: formOptions },
      };
    case 'search':
    case 'text':
      return {
        type: 'string',
        label: filter.label,
        placeholder: filter.label,
        control: { kind: 'text' },
      };
  }
}

export function reportRangeToControlValue(value: unknown): unknown[] {
  const range =
    value && typeof value === 'object' && !Array.isArray(value)
      ? (value as { min?: unknown; max?: unknown })
      : {};
  return [range.min ?? '', range.max ?? ''];
}

export function controlValueToReportRange(value: unknown): {
  min?: unknown;
  max?: unknown;
} {
  const range = Array.isArray(value) ? value : [];
  return {
    min: range[0] === '' ? undefined : range[0],
    max: range[1] === '' ? undefined : range[1],
  };
}

export function reportEditorToFormField(
  value: unknown,
  format: string | null | undefined,
  pillVariants: Partial<Record<string, string>> | null | undefined,
  editor: ReportEditorConfig | null | undefined
): FormField {
  let kind = editor?.kind;
  if (!kind) {
    if (
      format === 'pill' &&
      pillVariants &&
      Object.keys(pillVariants).length > 0
    ) {
      kind = 'select';
    } else if (
      ['currency', 'currency_compact', 'decimal', 'percent', 'number'].includes(
        format ?? ''
      )
    ) {
      kind = 'number';
    } else if (format === 'datetime' || format === 'date') {
      kind = format;
    } else if (typeof value === 'boolean') {
      kind = 'toggle';
    } else if (typeof value === 'number') {
      kind = 'number';
    } else {
      kind = 'text';
    }
  }
  const options =
    editor?.options ??
    Object.keys(pillVariants ?? {}).map((option) => ({
      label: option,
      value: option,
    }));
  return {
    type:
      kind === 'toggle' ? 'boolean' : kind === 'number' ? 'number' : 'string',
    min: editor?.min ?? undefined,
    max: editor?.max ?? undefined,
    pattern: editor?.regex ?? undefined,
    placeholder: editor?.placeholder ?? undefined,
    control: {
      kind,
      options,
    },
  };
}
