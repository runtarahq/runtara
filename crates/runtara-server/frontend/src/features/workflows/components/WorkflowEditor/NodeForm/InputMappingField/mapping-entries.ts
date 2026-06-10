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
 * Mapping entry extended with the editor-only `autoSeeded` marker.
 *
 * `autoSeeded: true` means the row was created by the editor (capability /
 * child-workflow schema population) and the user never entered a value. The
 * save path (cleanNodeData in CustomNodes/utils.tsx) drops such rows when
 * their value is still the empty string, while explicit immediate '' values
 * (loaded from the step JSON or typed by the user) are preserved — passing ""
 * to an input is legal DSL.
 */
export type SeededMappingEntry = InputMappingEntry & { autoSeeded?: boolean };

/**
 * Convert editor entries to the format stored in the react-hook-form
 * `inputMapping` field array.
 */
export function toFormMappingEntries(
  entries: SeededMappingEntry[]
): SeededMappingEntry[] {
  return entries.map((entry) => ({
    type: entry.type,
    value: entry.value,
    valueType: entry.valueType,
    typeHint: entry.typeHint,
    ...(entry.defaultValue !== undefined
      ? { defaultValue: entry.defaultValue }
      : {}),
    ...(entry.autoSeeded !== undefined ? { autoSeeded: entry.autoSeeded } : {}),
  }));
}

/**
 * Convert form field-array items to the editor's initial-data format.
 */
export function toEditorInitialData(
  items: Array<Partial<SeededMappingEntry> & { type: string }>
): SeededMappingEntry[] {
  return items.map((item) => ({
    type: item.type,
    value: item.value ?? '',
    valueType: item.valueType ?? 'immediate',
    typeHint: item.typeHint,
    ...(item.defaultValue !== undefined
      ? { defaultValue: item.defaultValue }
      : {}),
    ...(item.autoSeeded !== undefined ? { autoSeeded: item.autoSeeded } : {}),
  }));
}
