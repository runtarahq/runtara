/**
 * Whole-step JSON view of an input mapping — the escape hatch for everything
 * the structured editor cannot express.
 *
 * The structured editor is the right default, but it has holes: `any`-typed
 * roots that cannot become arrays, composites the row editor will not expand,
 * shapes authored through MCP that no widget models. Without a way out, the
 * only recourse was to leave the UI. `MappingObjectField` has had exactly this
 * toggle for Log/Error/WaitForSignal/compensation all along; this brings it to
 * the surface every Agent step uses.
 *
 * The JSON is keyed by field name and each value carries the editor's own
 * entry shape, so a round-trip through the textarea is lossless. Parsing also
 * accepts the DSL's `default` alongside the editor's `defaultValue`, which is
 * what makes pasting a mapping straight out of an MCP transcript work.
 */

import type { SeededMappingEntry } from './mapping-entries';

/** One field's value as rendered in, and accepted from, the JSON view. */
export interface MappingJsonValue {
  value: unknown;
  valueType?: string;
  typeHint?: string;
  defaultValue?: unknown;
}

export interface MappingJsonParseResult {
  entries: SeededMappingEntry[] | null;
  /** Human-readable reason the text could not be applied, or null. */
  error: string | null;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/** Editor entries → the object rendered in the JSON view. */
export function entriesToMappingJson(
  entries: readonly SeededMappingEntry[] | undefined
): Record<string, MappingJsonValue> {
  const out: Record<string, MappingJsonValue> = {};
  for (const entry of entries ?? []) {
    if (!entry?.type) continue;
    const value: MappingJsonValue = {
      value: entry.value,
      valueType: entry.valueType ?? 'immediate',
    };
    if (entry.typeHint !== undefined && entry.typeHint !== 'auto') {
      value.typeHint = entry.typeHint;
    }
    if (entry.defaultValue !== undefined) {
      value.defaultValue = entry.defaultValue;
    }
    out[entry.type] = value;
  }
  return out;
}

/** Pretty-printed JSON for the textarea. */
export function formatMappingJson(
  entries: readonly SeededMappingEntry[] | undefined
): string {
  return JSON.stringify(entriesToMappingJson(entries), null, 2);
}

/**
 * Parse the JSON view back into editor entries.
 *
 * Returns a specific, positioned message on failure rather than a flat
 * "invalid JSON" — the whole point of the hatch is that the author can see
 * what is wrong.
 */
export function parseMappingJson(text: string): MappingJsonParseResult {
  const trimmed = text.trim();
  if (!trimmed) return { entries: [], error: null };

  let parsed: unknown;
  try {
    parsed = JSON.parse(trimmed);
  } catch (error) {
    return {
      entries: null,
      error:
        error instanceof SyntaxError
          ? `Invalid JSON — ${error.message}`
          : 'Invalid JSON',
    };
  }

  if (!isPlainObject(parsed)) {
    return {
      entries: null,
      error:
        'Expected an object keyed by parameter name, e.g. {"schema_name": {"value": "Product", "valueType": "immediate"}}',
    };
  }

  const entries: SeededMappingEntry[] = [];
  for (const [name, raw] of Object.entries(parsed)) {
    if (!name) {
      return { entries: null, error: 'Parameter names cannot be empty' };
    }

    // A bare literal is a convenience shorthand for an immediate value.
    if (!isPlainObject(raw) || !('value' in raw)) {
      entries.push({
        type: name,
        value: raw as SeededMappingEntry['value'],
        valueType: 'immediate',
      } as SeededMappingEntry);
      continue;
    }

    const valueType = (raw.valueType as string | undefined) ?? 'immediate';
    if (
      !['immediate', 'reference', 'template', 'composite'].includes(valueType)
    ) {
      return {
        entries: null,
        error: `"${name}" has an unknown valueType "${valueType}" — expected immediate, reference, template or composite`,
      };
    }

    const entry: SeededMappingEntry = {
      type: name,
      value: raw.value as SeededMappingEntry['value'],
      valueType: valueType as SeededMappingEntry['valueType'],
    } as SeededMappingEntry;

    if (raw.typeHint !== undefined) {
      entry.typeHint = raw.typeHint as SeededMappingEntry['typeHint'];
    }
    // `defaultValue` is the editor's spelling, `default` the DSL's. Accept
    // both so a mapping pasted from MCP or the REST API applies as-is.
    const fallback =
      raw.defaultValue !== undefined ? raw.defaultValue : raw.default;
    if (fallback !== undefined) {
      entry.defaultValue = fallback;
    }

    entries.push(entry);
  }

  return { entries, error: null };
}
