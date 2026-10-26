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
 * parses to an object or array is stored as that structure, so it saves as
 * one even when it holds `{{` (the save path leaves such text unparsed).
 * Anything else — text that does not parse yet, or a JSON scalar — is held as
 * the raw text with the `json` type hint, which the save path
 * (processMappingEntry) parses and the node form's resolver checks, showing a
 * parse error under the row. Emptied, the input is unset and the save leaves
 * it out. A literal that is never edited is saved exactly as it was loaded.
 */

import { useRef, useState } from 'react';

export const JSON_TYPE_HINT = 'json';

export function isStructuredValue(value: unknown): value is object {
  return typeof value === 'object' && value !== null;
}

/**
 * Whether an entry is a JSON literal the editor should show as JSON text: an
 * immediate holding a structure, or the text of one being edited. `untyped`
 * fields only get the `json` hint from such an edit; typed object and array
 * fields carry it from the start, so for them an empty string is still the
 * untouched seed and opens in Build.
 */
export function isJsonLiteral(
  value: unknown,
  valueType: string,
  typeHint: string | undefined,
  untyped: boolean
): boolean {
  if (valueType !== 'immediate') return false;
  if (isStructuredValue(value)) return true;
  if (typeHint !== JSON_TYPE_HINT) return false;
  return untyped || (typeof value === 'string' && value !== '');
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
  untyped: boolean;
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
  untyped,
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
    isJsonLiteral(value, valueType, typeHint, untyped) ||
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
    if (structure === undefined && typeHint !== JSON_TYPE_HINT) {
      onTypeHintChange?.(JSON_TYPE_HINT);
    }
    onChange(stored);
  };

  /**
   * Call before a mode switch. Returning to Value brings the JSON box back for
   * a field that held a literal; leaving drops a `json` hint this editor set,
   * so an `any` field's plain value is not read as JSON.
   */
  const switchMode = (toValue: boolean) => {
    const keepJson = toValue && held.current;
    setDraft(undefined);
    setEditing(keepJson);
    if (keepJson) {
      if (typeHint !== JSON_TYPE_HINT) onTypeHintChange?.(JSON_TYPE_HINT);
    } else if (typeHint === JSON_TYPE_HINT) {
      onTypeHintChange?.(undefined);
    }
  };

  return { active, held: held.current, text, edit, switchMode };
}
