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

/* ------------------------------------------------------------------------ *
 * Mapping-object helpers (MappingObjectField)
 *
 * Four DSL fields are InputMapping-shaped objects (name → MappingValue)
 * held in form state as a single mapping-entry value with
 * valueType 'composite': Log.context, Error.context,
 * WaitForSignal.action.correlation and .context. Depending on how the value
 * got there it is either:
 *   - UI format (loaded via convertCompositeToUIFormat in CustomNodes/utils):
 *     entries carry `typeHint` / `defaultValue`;
 *   - raw DSL format (typed into the JSON textarea): entries carry
 *     `type` / `default`;
 *   - bare literals (no `valueType` wrapper at all).
 *
 * The save path (processCompositeValue in CustomNodes/utils.tsx) accepts all
 * three shapes, reading `typeHint ?? type` and `defaultValue ?? default`.
 * normalizeMappingObject below converts any accepted shape into UI format so
 * the structured editor can render it; writing the normalized object back to
 * form state therefore serializes to exactly the same DSL output.
 * ------------------------------------------------------------------------ */

export type MappingObjectValueType =
  | 'immediate'
  | 'reference'
  | 'template'
  | 'composite';

/** UI-format mapping value as edited by MappingObjectField rows. */
export type MappingObjectEntry = {
  valueType: MappingObjectValueType;
  value: unknown;
  typeHint?: string;
  defaultValue?: unknown;
};

const MAPPING_VALUE_TYPES: ReadonlySet<string> = new Set([
  'immediate',
  'reference',
  'template',
  'composite',
]);

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/**
 * Normalize a single mapping value (UI format, raw DSL format, or a bare
 * literal) into the UI format. Returns null for shapes the structured editor
 * cannot represent (unknown valueType discriminants) — callers fall back to
 * the JSON editor so exotic shapes stay reachable.
 */
export function normalizeMappingValue(raw: unknown): MappingObjectEntry | null {
  if (!isPlainObject(raw) || !('valueType' in raw)) {
    // Bare literal (string/number/boolean/null/plain object/array) — the
    // serializer wraps these as immediate values.
    return { valueType: 'immediate', value: raw === undefined ? '' : raw };
  }

  const valueType = raw.valueType;
  if (typeof valueType !== 'string' || !MAPPING_VALUE_TYPES.has(valueType)) {
    return null;
  }

  const typeHint = raw.typeHint ?? raw.type;

  if (valueType === 'composite') {
    const inner = raw.value;
    let normalizedInner: unknown;
    if (Array.isArray(inner)) {
      const items: MappingObjectEntry[] = [];
      for (const item of inner) {
        const normalized = normalizeMappingValue(item);
        if (normalized === null) return null;
        items.push(normalized);
      }
      normalizedInner = items;
    } else if (isPlainObject(inner)) {
      const obj: Record<string, MappingObjectEntry> = {};
      for (const [key, val] of Object.entries(inner)) {
        const normalized = normalizeMappingValue(val);
        if (normalized === null) return null;
        obj[key] = normalized;
      }
      normalizedInner = obj;
    } else {
      // The serializer treats non-object composite payloads as {}.
      normalizedInner = {};
    }
    return {
      valueType: 'composite',
      value: normalizedInner,
      ...(typeof typeHint === 'string' ? { typeHint } : {}),
    };
  }

  const entry: MappingObjectEntry = {
    valueType: valueType as MappingObjectValueType,
    value: raw.value === undefined ? '' : raw.value,
  };
  if (typeof typeHint === 'string') {
    entry.typeHint = typeHint;
  }
  // ReferenceValue.default — only references carry a fallback in the DSL.
  const defaultValue = raw.defaultValue ?? raw.default;
  if (valueType === 'reference' && defaultValue !== undefined) {
    entry.defaultValue = defaultValue;
  }
  return entry;
}

/**
 * Normalize a whole mapping object (name → MappingValue) into UI format.
 * Empty-ish inputs ('' / null / undefined — how the form represents an
 * absent field) normalize to {}. Non-object inputs (raw strings from invalid
 * JSON, arrays, scalars) and objects containing unrepresentable entries
 * return null, signalling "JSON editing only".
 */
export function normalizeMappingObject(
  raw: unknown
): Record<string, MappingObjectEntry> | null {
  if (raw === undefined || raw === null || raw === '') return {};
  if (!isPlainObject(raw)) return null;
  const out: Record<string, MappingObjectEntry> = {};
  for (const [key, val] of Object.entries(raw)) {
    const entry = normalizeMappingValue(val);
    if (entry === null) return null;
    out[key] = entry;
  }
  return out;
}

/**
 * Parse JSON-textarea input the way the legacy mapping-object textareas did:
 * blank → {} (clears the field on save), valid JSON → parsed value, invalid
 * JSON → the raw string (kept so the user can keep typing; the serializer
 * drops non-object values).
 */
export function parseMappingObjectJson(text: string): unknown {
  if (!text.trim()) return {};
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

/**
 * Render a mapping-object form value for the JSON textarea (legacy
 * getJsonValue behavior): falsy → '', strings verbatim, else pretty JSON.
 */
export function formatMappingObjectJson(value: unknown): string {
  if (!value) return '';
  return typeof value === 'string' ? value : JSON.stringify(value, null, 2);
}
