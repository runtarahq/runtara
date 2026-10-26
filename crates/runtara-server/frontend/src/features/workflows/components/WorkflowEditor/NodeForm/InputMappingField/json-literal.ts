/**
 * An immediate whose value is a structure — an object or array authored via
 * MCP or the REST API, such as an Object Model query `condition` — is a single
 * literal, not a build.
 *
 * The Build editor works on `{valueType, value}` entries at every level, so
 * reading such a value as a build showed every bare property as "Unknown
 * type", and the first edit there rewrote the field as `composite`. That is a
 * different shape: the server rejects it for condition inputs (E025), and at
 * run time a composite resolves each `{valueType: "reference"}` it holds as a
 * workflow path, while an Object Model condition's first argument names a
 * column.
 *
 * So the editors keep it an immediate and edit it as JSON text. Text that
 * parses to an object or array is stored as that structure. Anything else —
 * text that does not parse yet, or a JSON scalar — is held as the raw text
 * under the JSON box's own type hint (JSON_LITERAL_HINT), which the save path
 * (processMappingEntry) parses and the node form's resolver checks, showing a
 * parse error under the row. Under that hint the text is JSON even when it
 * holds `{{`: `"Hello {{name}}"` saves as a string, not as quoted text.
 * Emptied, the input is unset and the save leaves it out. A literal that is
 * never edited is saved exactly as it was loaded.
 */

import { useRef, useState } from 'react';
import { JSON_LITERAL_HINT } from '@/features/workflows/utils/mapping-serialization';

export function isStructuredValue(value: unknown): value is object {
  return typeof value === 'object' && value !== null;
}

/**
 * Whether an entry is a JSON literal the editor should show as JSON text: an
 * immediate holding a structure, or text from the JSON box. Only the JSON box
 * sets its hint, so an empty string under it is a literal being written,
 * while a typed object field's untouched seed ('' under `json`) opens in
 * Build.
 */
export function isJsonLiteral(
  value: unknown,
  valueType: string,
  typeHint: string | undefined
): boolean {
  if (valueType !== 'immediate') return false;
  return isStructuredValue(value) || typeHint === JSON_LITERAL_HINT;
}

/** The text a JSON literal is edited as. */
export function jsonLiteralText(value: unknown): string {
  if (isStructuredValue(value)) return JSON.stringify(value, null, 2);
  return value === null || value === undefined ? '' : String(value);
}

/** The object or array `text` parses to, or undefined. */
export function parseJsonStructure(text: string): object | undefined {
  try {
    const parsed: unknown = JSON.parse(text);
    return isStructuredValue(parsed) ? parsed : undefined;
  } catch {
    return undefined;
  }
}

interface JsonLiteralOptions {
  value: unknown;
  valueType: string;
  typeHint: string | undefined;
  onChange: (value: string | object) => void;
  onTypeHintChange?: (typeHint: string | undefined) => void;
}

/**
 * The JSON-literal state an inline object or array editor shares: whether to
 * show the JSON box, the text in it, and what an edit or a mode switch does.
 */
export function useJsonLiteral({
  value,
  valueType,
  typeHint,
  onChange,
  onTypeHintChange,
}: JsonLiteralOptions) {
  // Set once the author edits the literal, so clearing the text to retype it
  // does not drop a typed field back to an empty Build.
  const [editing, setEditing] = useState(false);
  // The text as typed, with the value it stored. A parsed structure would
  // otherwise be re-printed on every keystroke, moving the cursor.
  const [draft, setDraft] = useState<{ text: string; stored: unknown }>();
  // Whether the field held a literal while this editor was open, so a mode
  // switch away and back can return to the JSON box.
  const held = useRef(false);

  const active =
    isJsonLiteral(value, valueType, typeHint) ||
    (editing && valueType === 'immediate');
  if (active) held.current = true;

  const text =
    draft !== undefined &&
    JSON.stringify(draft.stored) === JSON.stringify(value)
      ? draft.text
      : jsonLiteralText(value);

  const edit = (next: string) => {
    setEditing(true);
    const structure = parseJsonStructure(next);
    const stored = structure ?? next;
    setDraft({ text: next, stored });
    if (structure === undefined && typeHint !== JSON_LITERAL_HINT) {
      onTypeHintChange?.(JSON_LITERAL_HINT);
    }
    onChange(stored);
  };

  /**
   * Call before a mode switch. Returning to Value brings the JSON box back for
   * a field that held a literal; leaving drops the JSON box's hint, so the
   * field's own hint applies to what comes next.
   */
  const switchMode = (toValue: boolean) => {
    const keepJson = toValue && held.current;
    setDraft(undefined);
    setEditing(keepJson);
    if (keepJson) {
      if (typeHint !== JSON_LITERAL_HINT) onTypeHintChange?.(JSON_LITERAL_HINT);
    } else if (typeHint === JSON_LITERAL_HINT) {
      onTypeHintChange?.(undefined);
    }
  };

  return { active, held: held.current, text, edit, switchMode };
}
