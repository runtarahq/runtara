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
 * So the editors keep it an immediate and edit it as JSON text. While it is
 * being edited the field holds the raw text with the `json` type hint, which
 * the save path (processMappingEntry) parses back into a structure and the
 * node form's resolver checks, showing a parse error under the row. A literal
 * that is never edited is saved exactly as it was loaded.
 */

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
