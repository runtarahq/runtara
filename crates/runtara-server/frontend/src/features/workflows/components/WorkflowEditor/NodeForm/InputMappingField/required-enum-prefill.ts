/**
 * Pre-fill for required string-enum capability inputs.
 *
 * A required input whose schema is a string enum with no default (e.g. the
 * control agent's `start.parentClosePolicy`, enum ["cancel", "leave_running"])
 * has no neutral empty value: leaving it unmapped fails validation (E022).
 * The step editor therefore writes the enum's first value into the step's
 * inputMapping as an immediate value — exactly what the user picking that
 * option would store — the first time such a field is shown without a
 * mapping. Optional enums, fields with a default and fields that already
 * carry a mapping (any value type, including a reference) are left alone.
 */

import type { SeededMappingEntry } from './mapping-entries';

/** The capability-field metadata this decision needs. */
export type EnumPrefillField = {
  name: string;
  type?: string;
  required?: boolean;
  default?: unknown;
  enum?: unknown[] | null;
};

/**
 * The value a required string-enum field without a default is pre-filled
 * with (its first enum value), or undefined when the field does not qualify.
 */
export function getRequiredEnumPrefill(
  field: EnumPrefillField
): string | undefined {
  if (!field.required) return undefined;
  if (field.default !== undefined && field.default !== null) return undefined;
  const first = field.enum?.[0];
  return typeof first === 'string' ? first : undefined;
}

/**
 * True when the field has no real mapping: no entry at all, or only the
 * untouched empty row the editor auto-seeds from the schema (which the save
 * path drops).
 */
export function isUnsetMappingEntry(
  entry: Partial<SeededMappingEntry> | undefined
): boolean {
  if (!entry) return true;
  return (
    entry.autoSeeded === true &&
    (entry.valueType ?? 'immediate') === 'immediate' &&
    (entry.value === undefined || entry.value === null || entry.value === '')
  );
}

/**
 * Apply the required-enum pre-fill to a mapping-entry list.
 *
 * Returns the updated entries plus the names of the fields that were
 * pre-filled, or null when nothing changes. Fields listed in `skip` (already
 * pre-filled once for this step) are never touched again so a later user
 * choice is not overwritten.
 */
export function applyRequiredEnumPrefills(
  fields: EnumPrefillField[],
  entries: Array<Partial<SeededMappingEntry> & { type: string }>,
  getTypeHint: (field: EnumPrefillField) => string,
  skip: ReadonlySet<string> = new Set()
): {
  entries: Array<Partial<SeededMappingEntry> & { type: string }>;
  prefilled: string[];
} | null {
  const next = [...entries];
  const prefilled: string[] = [];

  for (const field of fields) {
    if (skip.has(field.name)) continue;
    const value = getRequiredEnumPrefill(field);
    if (value === undefined) continue;

    const index = next.findIndex((entry) => entry.type === field.name);
    if (!isUnsetMappingEntry(index === -1 ? undefined : next[index])) continue;

    const entry = {
      type: field.name,
      value,
      valueType: 'immediate' as const,
      typeHint: getTypeHint(field),
    };
    if (index === -1) {
      next.push(entry);
    } else {
      next[index] = entry;
    }
    prefilled.push(field.name);
  }

  return prefilled.length > 0 ? { entries: next, prefilled } : null;
}
