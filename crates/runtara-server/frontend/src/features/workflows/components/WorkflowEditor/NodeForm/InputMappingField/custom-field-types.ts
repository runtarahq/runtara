/**
 * Type options offered when adding a custom parameter — a parameter the
 * capability schema does not declare.
 *
 * Values are API `ValueType`s, and that vocabulary has no separate `array`:
 * `VALID_VALUE_TYPES` in `CustomNodes/utils.tsx` is
 * {string, integer, number, boolean, json, file}, and `isValidValueType`
 * drops any hint outside it on save. `json` therefore covers both objects and
 * arrays.
 *
 * There used to be two entries here — `{value: 'json', label: 'JSON Object'}`
 * and `{value: 'json', label: 'Array'}`. Because a Radix Select renders the
 * text of *every* item matching the current value, picking either one made the
 * trigger read "JSON ObjectArray", and the two options were indistinguishable
 * in effect. One honest option replaces both. (Adding a real `array` ValueType
 * would mean widening VALID_VALUE_TYPES and the save path with it — a
 * different, larger change.)
 *
 * This list was duplicated verbatim in CustomFieldRow and
 * AddCustomFieldDialog; both now import it so they cannot drift.
 */
export interface CustomFieldTypeOption {
  value: string;
  label: string;
}

export const CUSTOM_FIELD_TYPES: readonly CustomFieldTypeOption[] = [
  { value: 'string', label: 'String' },
  { value: 'integer', label: 'Integer' },
  { value: 'number', label: 'Number' },
  { value: 'boolean', label: 'Boolean' },
  { value: 'json', label: 'JSON (object or array)' },
  { value: 'file', label: 'File' },
];

/** Label for a stored type hint, falling back to the raw hint. */
export function customFieldTypeLabel(value: string | undefined): string {
  if (!value) return '';
  return CUSTOM_FIELD_TYPES.find((t) => t.value === value)?.label ?? value;
}
