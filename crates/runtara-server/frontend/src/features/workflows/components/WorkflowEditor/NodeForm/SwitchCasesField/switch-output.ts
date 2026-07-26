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

/** Interpret a stored case output for the editor. */
export function readSwitchOutput(output: unknown): SwitchOutputValue {
  if (isPlainObject(output) && typeof output.valueType === 'string') {
    if (output.valueType === 'reference') {
      return {
        mode: 'reference',
        value: typeof output.value === 'string' ? output.value : '',
      };
    }
    if (output.valueType === 'immediate') {
      return { mode: 'immediate', value: output.value ?? '' };
    }
    // Any other wrapper is, to the runtime, just an object.
    return { mode: 'composite', value: output };
  }

  if (isPlainObject(output) || Array.isArray(output)) {
    return { mode: 'composite', value: output };
  }

  return { mode: 'immediate', value: output ?? '' };
}

/**
 * Serialize the editor's state back to the stored shape.
 *
 * Immediate scalars stay bare rather than gaining a wrapper — that is what the
 * runtime's literal arm expects and what every existing workflow already
 * stores, so editing an unrelated case does not rewrite this one.
 */
export function writeSwitchOutput({ mode, value }: SwitchOutputValue): unknown {
  if (mode === 'reference') {
    return {
      valueType: 'reference',
      value: typeof value === 'string' ? value : '',
    };
  }
  if (mode === 'composite') {
    return isPlainObject(value) || Array.isArray(value) ? value : {};
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
