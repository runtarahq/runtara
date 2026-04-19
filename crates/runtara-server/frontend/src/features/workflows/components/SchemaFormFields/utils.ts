import { type SchemaField } from '@/features/workflows/utils/schema';

/** Humanize a camelCase/snake_case field key into a label. */
export function humanizeKey(key: string): string {
  return key
    .replace(/([a-z])([A-Z])/g, '$1 $2')
    .replace(/_/g, ' ')
    .replace(/\b\w/g, (c) => c.toUpperCase());
}

/** Check if a field should be visible based on visibleWhen condition. */
export function isFieldVisible(
  field: SchemaField,
  formValues: Record<string, any>
): boolean {
  if (!field.visibleWhen) return true;
  const { field: depField, equals, notEquals } = field.visibleWhen;
  const depValue = formValues[depField];
  if (equals !== undefined) return depValue === equals;
  if (notEquals !== undefined) return depValue !== notEquals;
  return true;
}
