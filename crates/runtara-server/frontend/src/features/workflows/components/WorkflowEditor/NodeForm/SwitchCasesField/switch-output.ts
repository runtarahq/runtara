/**
 * Reading and writing a Switch case's `output`.
 *
 * The stored shape is a bare `serde_json::Value` (SwitchCase.output in
 * runtara-dsl/src/schema_types.rs) and the runtime interprets it in
 * `process_switch_output` (runtara-workflow-stdlib/src/switch_helpers.rs):
 *
 *   - a literal scalar is returned as-is;
 *   - `{valueType: "reference", value: "dot.path"}` resolves the path against
 *     the workflow scope;
 *   - `{valueType: "immediate", value: X}` yields X;
 *   - any other object or array is recursed, so nested reference wrappers
 *     inside a structure resolve too.
 *
 * Two consequences the editor has to respect:
 *
 *   - **There is no template support.** A `{valueType: "template"}` wrapper
 *     falls through to the recurse arm and is emitted as a literal object, so
 *     offering template mode here would let someone build something that
 *     silently does not work.
 *   - **References are plain dot paths.** `resolve_dot_path` splits on `.`
 *     only, so bracket syntax (`steps['a'].outputs`) does not resolve.
 */

export type SwitchOutputMode = 'immediate' | 'reference' | 'composite';

/** Modes the Switch runtime actually honours, in toggle order. */
export const SWITCH_OUTPUT_MODES: readonly SwitchOutputMode[] = [
  'immediate',
  'reference',
  'composite',
];

export interface SwitchOutputValue {
  mode: SwitchOutputMode;
  /** Scalar/path for immediate and reference; the structure for composite. */
  value: unknown;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/**
 * The composite editor (ObjectMappingEditor / CompositeValueItem) works on
 * `{valueType, value}` entries at every level, while the stored structure may
 * hold bare literals and nested plain objects, which the runtime passes
 * through or recurses. These helpers translate between the two, so a literal
 * `{queue: "web-team"}` opens as one immediate field instead of an
 * "Unknown type" error.
 */
type EditorEntry = { valueType: string; value: unknown; typeHint?: string };

function isRuntimeWrapper(
  value: unknown
): value is { valueType: 'reference' | 'immediate'; value: unknown } {
  return (
    isPlainObject(value) &&
    (value.valueType === 'reference' || value.valueType === 'immediate') &&
    'value' in value
  );
}

/**
 * The editor's own nested-structure wrapper. The runtime has no such arm and
 * would emit it literally, so it is read as the structure the author built.
 */
function isCompositeWrapper(
  value: unknown
): value is { valueType: 'composite'; value: unknown } {
  return (
    isPlainObject(value) && value.valueType === 'composite' && 'value' in value
  );
}

function mapStructure(
  value: unknown,
  map: (entry: unknown) => unknown
): unknown {
  if (Array.isArray(value)) return value.map(map);
  if (isPlainObject(value)) {
    return Object.fromEntries(
      Object.entries(value).map(([key, entry]) => [key, map(entry)])
    );
  }
  return value;
}

/**
 * An immediate entry. A string the editor's "auto" hint would re-read as a
 * number or boolean ("123", "true") keeps an explicit string hint, so editing
 * it does not silently retype it.
 */
function immediateEntry(value: unknown): EditorEntry {
  const looksTyped =
    typeof value === 'string' &&
    (value === 'true' ||
      value === 'false' ||
      (value.trim() !== '' && !Number.isNaN(Number(value))));
  return looksTyped
    ? { valueType: 'immediate', value, typeHint: 'string' }
    : { valueType: 'immediate', value };
}

function toEditorEntry(stored: unknown): EditorEntry {
  if (isRuntimeWrapper(stored)) {
    return stored.valueType === 'reference'
      ? {
          valueType: 'reference',
          value: typeof stored.value === 'string' ? stored.value : '',
        }
      : immediateEntry(stored.value);
  }
  if (isCompositeWrapper(stored)) return toEditorEntry(stored.value);
  if (isPlainObject(stored) || Array.isArray(stored)) {
    return {
      valueType: 'composite',
      value: mapStructure(stored, toEditorEntry),
    };
  }
  return immediateEntry(stored ?? null);
}

/** Apply an immediate's type hint, which the composite editor defers. */
function coerceImmediate(value: unknown, typeHint?: string): unknown {
  if (typeof value !== 'string') return value;
  if ((typeHint === 'integer' || typeHint === 'number') && value.trim()) {
    const parsed = Number(value);
    return Number.isNaN(parsed) ? value : parsed;
  }
  if (typeHint === 'json') {
    try {
      return JSON.parse(value);
    } catch {
      return value;
    }
  }
  return value;
}

function fromEditorEntry(entry: unknown): unknown {
  if (!isPlainObject(entry) || typeof entry.valueType !== 'string') {
    return entry;
  }
  const { valueType, value, typeHint } = entry as EditorEntry;
  if (valueType === 'reference') {
    return {
      valueType: 'reference',
      value: typeof value === 'string' ? value : '',
    };
  }
  if (valueType === 'composite') return mapStructure(value, fromEditorEntry);
  if (valueType === 'immediate') {
    const coerced = coerceImmediate(value, typeHint);
    // Scalars stay bare, as the runtime's literal arm expects. A structured
    // immediate keeps its wrapper so the runtime returns it unrecursed.
    return isPlainObject(coerced) || Array.isArray(coerced)
      ? { valueType: 'immediate', value: coerced }
      : coerced;
  }
  return entry;
}

/** Interpret a stored case output for the editor. */
export function readSwitchOutput(output: unknown): SwitchOutputValue {
  if (isRuntimeWrapper(output)) {
    if (output.valueType === 'reference') {
      return {
        mode: 'reference',
        value: typeof output.value === 'string' ? output.value : '',
      };
    }
    return { mode: 'immediate', value: output.value ?? '' };
  }

  const structure = isCompositeWrapper(output) ? output.value : output;
  if (isPlainObject(structure) || Array.isArray(structure)) {
    return { mode: 'composite', value: mapStructure(structure, toEditorEntry) };
  }

  return { mode: 'immediate', value: output ?? '' };
}

/**
 * Serialize the editor's state back to the stored shape.
 *
 * Immediate scalars stay bare rather than gaining a wrapper — that is what the
 * runtime's literal arm expects and what every existing workflow already
 * stores, so editing an unrelated case does not rewrite this one. Composite
 * entries are unwrapped back to plain structure for the same reason.
 */
export function writeSwitchOutput({ mode, value }: SwitchOutputValue): unknown {
  if (mode === 'reference') {
    return {
      valueType: 'reference',
      value: typeof value === 'string' ? value : '',
    };
  }
  if (mode === 'composite') {
    return isPlainObject(value) || Array.isArray(value)
      ? mapStructure(value, fromEditorEntry)
      : {};
  }
  return value ?? '';
}

/** One-line summary for a collapsed composite output cell. */
export function describeSwitchOutput(output: unknown): string {
  const { mode, value } = readSwitchOutput(output);
  if (mode === 'reference') return `Reference: ${value || '(not set)'}`;
  if (mode === 'composite') {
    if (Array.isArray(value)) {
      return `${value.length} item${value.length === 1 ? '' : 's'}`;
    }
    const count = Object.keys(value as Record<string, unknown>).length;
    return `${count} field${count === 1 ? '' : 's'}`;
  }
  if (value === '' || value === null || value === undefined) return '(empty)';
  return String(value);
}
