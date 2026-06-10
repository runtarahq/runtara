/**
 * Pure helpers for converting input mapping entries between the
 * SimpleInputMappingEditor (Zustand store) shape and the react-hook-form
 * field-array shape.
 *
 * Both directions must preserve `defaultValue` (ReferenceValue.default):
 * dropping it here would destroy a JSON-authored fallback on the next
 * node-form save even though the plain save/load path round-trips it.
 */

import type { InputMappingEntry } from '@/features/workflows/stores/nodeFormStore';

/**
 * Convert editor entries to the format stored in the react-hook-form
 * `inputMapping` field array.
 */
export function toFormMappingEntries(
  entries: InputMappingEntry[]
): InputMappingEntry[] {
  return entries.map((entry) => ({
    type: entry.type,
    value: entry.value,
    valueType: entry.valueType,
    typeHint: entry.typeHint,
    ...(entry.defaultValue !== undefined
      ? { defaultValue: entry.defaultValue }
      : {}),
  }));
}

/**
 * Convert form field-array items to the editor's initial-data format.
 */
export function toEditorInitialData(
  items: Array<Partial<InputMappingEntry> & { type: string }>
): InputMappingEntry[] {
  return items.map((item) => ({
    type: item.type,
    value: item.value ?? '',
    valueType: item.valueType ?? 'immediate',
    typeHint: item.typeHint,
    ...(item.defaultValue !== undefined
      ? { defaultValue: item.defaultValue }
      : {}),
  }));
}
