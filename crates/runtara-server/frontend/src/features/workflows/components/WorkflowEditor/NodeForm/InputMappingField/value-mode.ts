/**
 * Value carried across a mapping-value mode switch.
 *
 * Cycling immediate → template → reference → composite → immediate used to
 * call `onChange('')` on every transition, writing straight through to the
 * store. One stray click on a small unlabelled icon therefore deleted a saved
 * reference path or a hand-written literal, with no undo anywhere in the
 * editor.
 *
 * Nothing about a mode switch expresses an intent to discard. The four modes
 * are different *interpretations* of a value, and three of them
 * (immediate/template/reference) are plain strings, so the text carries over
 * unchanged. Composite is the only structural boundary, and even there a JSON
 * round-trip preserves the content in an editable form.
 *
 * Where the carried-over text is not meaningful in the destination mode — a
 * `Hello {{name}}` template read as a reference path — the reference validator
 * marks it in place. Showing a wrong value the author can fix beats silently
 * showing an empty one.
 */

export type ValueMode = 'immediate' | 'reference' | 'template' | 'composite';

/** Modes whose value is a plain string. */
const STRING_MODES: ReadonlySet<ValueMode> = new Set<ValueMode>([
  'immediate',
  'reference',
  'template',
]);

function isArrayFieldType(fieldType: string | undefined): boolean {
  if (!fieldType) return false;
  const lower = fieldType.toLowerCase();
  return (
    lower === 'array' ||
    lower.startsWith('array<') ||
    lower.startsWith('[') ||
    lower.includes('[]')
  );
}

/**
 * The value to write when switching from `from` to `to`.
 *
 * Returns `undefined` when the value should be left exactly as it is (no
 * `onChange` call at all) — the common case for string↔string transitions,
 * where re-writing the same value would only dirty the form.
 */
export function coerceValueForMode(
  value: unknown,
  from: ValueMode,
  to: ValueMode,
  fieldType?: string
): { changed: boolean; value: unknown } {
  if (from === to) return { changed: false, value };

  // String ↔ string: carry the text over untouched.
  if (STRING_MODES.has(from) && STRING_MODES.has(to)) {
    return { changed: false, value };
  }

  // Entering composite: keep an existing structure, otherwise seed an empty
  // one matching the field's shape. A non-empty scalar is preserved as the
  // single entry of the new structure rather than dropped.
  if (to === 'composite') {
    if (isStructural(value)) return { changed: false, value };
    return { changed: true, value: isArrayFieldType(fieldType) ? [] : {} };
  }

  // Leaving composite for a string mode: serialize rather than discard, so
  // the content stays visible and editable.
  if (from === 'composite') {
    if (!isStructural(value)) return { changed: true, value: '' };
    if (isEmptyStructure(value)) return { changed: true, value: '' };
    return { changed: true, value: JSON.stringify(value) };
  }

  return { changed: false, value };
}

function isStructural(value: unknown): boolean {
  return typeof value === 'object' && value !== null;
}

function isEmptyStructure(value: unknown): boolean {
  if (Array.isArray(value)) return value.length === 0;
  if (isStructural(value)) {
    return Object.keys(value as Record<string, unknown>).length === 0;
  }
  return false;
}

/** Next mode in the toggle cycle. */
export function nextValueMode(current: ValueMode): ValueMode {
  switch (current) {
    case 'immediate':
      return 'template';
    case 'template':
      return 'reference';
    case 'reference':
      return 'composite';
    default:
      return 'immediate';
  }
}
