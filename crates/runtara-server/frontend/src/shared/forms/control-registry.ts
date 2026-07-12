import type { FormControlKind, FormField, FormOption } from './types';

export function inferControlKind(field: FormField): FormControlKind {
  if (field.control?.kind) return field.control.kind;
  if (field.secret)
    return field.format === 'textarea' ? 'secret_textarea' : 'password';
  if (field.enum?.length)
    return field.type === 'array' ? 'multi_select' : 'select';
  if (field.format === 'textarea' || field.format === 'markdown')
    return 'textarea';
  if (field.format === 'date') return 'date';
  if (field.format === 'datetime' || field.format === 'date-time')
    return 'datetime';
  if (field.type === 'boolean') return 'toggle';
  if (field.type === 'integer' || field.type === 'number') return 'number';
  if (field.type === 'array') return 'tags';
  if (field.type === 'object') return 'key_value';
  if (field.type === 'file') return 'file';
  return 'text';
}

export function optionsFor(field: FormField): FormOption[] {
  if (field.control?.options?.length) return field.control.options;
  return (field.enum ?? []).map((value) => ({ value, label: String(value) }));
}
