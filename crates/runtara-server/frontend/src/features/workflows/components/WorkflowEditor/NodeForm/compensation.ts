/**
 * Pure helpers for the structured Agent.compensation editor (StepAdvancedFields
 * in NodeFormItem.tsx).
 *
 * The `compensation` form value passes through cleanNodeData (CustomNodes/
 * utils.tsx) verbatim for Agent steps — there is no save-time serializer hook
 * like Log/Error context have. The form value must therefore always hold the
 * raw DSL shape of `CompensationConfig` (runtara-dsl/src/schema_types.rs):
 *
 *   {
 *     compensationStep: string,            // required
 *     compensationData?: InputMapping,     // name -> MappingValue
 *     trigger?: string,                    // "on_downstream_error" (default),
 *                                          // "on_any_error", "manual"
 *     order?: number,                      // i32
 *   }
 *
 * The struct is `deny_unknown_fields`, but unknown keys present in the current
 * value (typed via the JSON fallback) are preserved on structured edits so the
 * structured controls never silently destroy JSON-authored content — the
 * backend rejects them with a clear serde error instead.
 *
 * serializeCompensationData mirrors processCompositeValue/processEntry in
 * CustomNodes/utils.tsx (the save path used by every other MappingObjectField
 * consumer) so structured compensationData edits serialize to exactly the DSL
 * shape the legacy JSON textarea produced.
 */

import {
  normalizeMappingObject,
  type MappingObjectEntry,
} from './InputMappingField/mapping-entries';

/** Documented `CompensationConfig.trigger` values (schema_types.rs:425-427). */
export const COMPENSATION_TRIGGER_OPTIONS: ReadonlyArray<{
  value: string;
  label: string;
}> = [
  { value: 'on_downstream_error', label: 'On downstream error' },
  { value: 'on_any_error', label: 'On any error' },
  { value: 'manual', label: 'Manual' },
];

const COMPENSATION_FIELDS = [
  'compensationStep',
  'trigger',
  'order',
  'compensationData',
] as const;

export type CompensationField = (typeof COMPENSATION_FIELDS)[number];

export type CompensationPatch = Partial<Record<CompensationField, unknown>>;

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/**
 * Extract the structured-editable parts from the current compensation form
 * value. Non-object values (which the legacy textarea could not produce, but
 * z.any() permits) read as fully empty.
 */
export function readCompensationParts(value: unknown): {
  compensationStep: string;
  trigger: string;
  order: number | '';
  compensationData: unknown;
} {
  if (!isPlainObject(value)) {
    return {
      compensationStep: '',
      trigger: '',
      order: '',
      compensationData: undefined,
    };
  }
  return {
    compensationStep:
      typeof value.compensationStep === 'string' ? value.compensationStep : '',
    trigger: typeof value.trigger === 'string' ? value.trigger : '',
    order: typeof value.order === 'number' ? value.order : '',
    compensationData: value.compensationData,
  };
}

/**
 * Apply a structured-field patch to the current compensation value.
 *
 * - Only keys present in `patch` are touched; unknown keys in `current`
 *   (JSON-authored) are preserved.
 * - Empty values ('' / null / undefined / empty compensationData object)
 *   remove the field — optionals are omitted, matching the
 *   skip_serializing_if behavior of CompensationConfig.
 * - When nothing remains, returns undefined so the `compensation` key is
 *   dropped from the saved step entirely (same as clearing the legacy
 *   textarea, which called form.setValue('compensation', undefined)).
 */
export function patchCompensation(
  current: unknown,
  patch: CompensationPatch
): Record<string, unknown> | undefined {
  const base: Record<string, unknown> = isPlainObject(current)
    ? { ...current }
    : {};

  for (const field of COMPENSATION_FIELDS) {
    if (!(field in patch)) continue;
    const value = patch[field];
    const isEmpty =
      value === undefined ||
      value === null ||
      value === '' ||
      (field === 'compensationData' &&
        isPlainObject(value) &&
        Object.keys(value).length === 0);
    if (isEmpty) {
      delete base[field];
    } else {
      base[field] = value;
    }
  }

  return Object.keys(base).length > 0 ? base : undefined;
}

/* ------------------------------------------------------------------------ *
 * compensationData serialization (UI format -> DSL InputMapping)
 *
 * Mirror of processCompositeValue's processEntry in CustomNodes/utils.tsx
 * (not exported there). Keep the two in sync: immediate values coerce by
 * typeHint, only reference values carry backend `type` / `default`, composite
 * values recurse, template values pass through.
 * ------------------------------------------------------------------------ */

/** Backend ValueType values accepted as a ReferenceValue type hint. */
const VALID_REFERENCE_TYPE_HINTS: ReadonlySet<string> = new Set([
  'string',
  'integer',
  'number',
  'boolean',
  'json',
  'file',
]);

/** Mirror of coerceValueToType in CustomNodes/utils.tsx. */
function coerceValueToType(value: unknown, typeHint?: string): unknown {
  if (typeHint === 'integer' || typeHint === 'number') {
    const numValue = Number(value);
    if (!isNaN(numValue)) {
      return typeHint === 'integer' ? Math.trunc(numValue) : numValue;
    }
  }
  if (typeHint === 'boolean' && typeof value === 'string') {
    const lower = value.toLowerCase();
    if (lower === 'true' || lower === '1') return true;
    if (lower === 'false' || lower === '0') return false;
  }
  return value;
}

function serializeUiEntry(entry: MappingObjectEntry): Record<string, unknown> {
  if (entry.valueType === 'composite') {
    const inner = entry.value;
    if (Array.isArray(inner)) {
      return {
        valueType: 'composite',
        value: inner.map((item) => serializeUiEntry(item as MappingObjectEntry)),
      };
    }
    if (isPlainObject(inner)) {
      const out: Record<string, unknown> = {};
      for (const [key, val] of Object.entries(inner)) {
        out[key] = serializeUiEntry(val as MappingObjectEntry);
      }
      return { valueType: 'composite', value: out };
    }
    return { valueType: 'composite', value: {} };
  }

  const coercedValue =
    entry.valueType === 'immediate' && entry.typeHint && entry.value !== null
      ? coerceValueToType(entry.value, entry.typeHint)
      : entry.value === undefined
        ? ''
        : entry.value;

  const out: Record<string, unknown> = {
    valueType: entry.valueType || 'immediate',
    value: coercedValue,
  };
  if (
    entry.valueType === 'reference' &&
    entry.typeHint !== undefined &&
    VALID_REFERENCE_TYPE_HINTS.has(entry.typeHint)
  ) {
    out.type = entry.typeHint;
  }
  if (entry.valueType === 'reference' && entry.defaultValue !== undefined) {
    out.default = entry.defaultValue;
  }
  return out;
}

export type CompensationDataResult =
  | {
      ok: true;
      /** DSL-format mapping object, or undefined when empty (omit the key). */
      data: Record<string, unknown> | undefined;
    }
  | {
      /** Not committable (e.g. an invalid-JSON string mid-typing). */
      ok: false;
    };

/**
 * Convert a MappingObjectField value (UI-format entries, raw DSL entries
 * loaded from the step JSON, or whatever its inner JSON textarea parsed to)
 * into the DSL InputMapping shape for CompensationConfig.compensationData.
 *
 * Idempotent over already-DSL values, so loaded data round-trips byte-equal.
 * Empty objects / blank input collapse to undefined (omit the optional key).
 * Plain objects the structured editor can't represent pass through verbatim
 * (legacy textarea parity — serde validates server-side). Everything else
 * (invalid JSON strings, arrays, scalars) is reported as not committable.
 */
export function serializeCompensationData(raw: unknown): CompensationDataResult {
  if (raw === undefined || raw === null || raw === '') {
    return { ok: true, data: undefined };
  }

  if (typeof raw === 'string') {
    if (!raw.trim()) return { ok: true, data: undefined };
    let parsed: unknown;
    try {
      parsed = JSON.parse(raw);
    } catch {
      return { ok: false };
    }
    if (!isPlainObject(parsed)) return { ok: false };
    return serializeCompensationData(parsed);
  }

  if (!isPlainObject(raw)) return { ok: false };

  const normalized = normalizeMappingObject(raw);
  if (normalized === null) {
    // Exotic shape (unknown valueType discriminants) typed via JSON —
    // pass through unchanged like the legacy compensation textarea did.
    return { ok: true, data: raw };
  }

  const keys = Object.keys(normalized);
  if (keys.length === 0) return { ok: true, data: undefined };

  const out: Record<string, unknown> = {};
  for (const key of keys) {
    out[key] = serializeUiEntry(normalized[key]);
  }
  return { ok: true, data: out };
}
