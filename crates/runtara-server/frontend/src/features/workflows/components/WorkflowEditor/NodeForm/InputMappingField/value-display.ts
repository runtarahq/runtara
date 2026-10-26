/**
 * Summary text for array/object mapping values shown in the collapsed row of
 * `SimpleInputMappingEditor`.
 *
 * The subtle case these exist to get right: an immediate array or object
 * authored via MCP or the REST API arrives as a *real JS array/object*, not as
 * a JSON string. `String(["sku"])` is `"sku"` and `String({a: 1})` is
 * `"[object Object]"` — neither parses, so a value-bearing field used to
 * summarize as "Click to configure...", i.e. indistinguishable from empty.
 */

import { describeCondition, isConditionExpression } from '../../step-details';

type ValueTypeName = string;

/** Pluralized "N item(s)". */
function items(count: number): string {
  return `${count} item${count !== 1 ? 's' : ''}`;
}

/** Pluralized "N field(s)". */
function fields(count: number): string {
  return `${count} field${count !== 1 ? 's' : ''}`;
}

export const EMPTY_VALUE_LABEL = 'Click to configure...';

export function describeArrayValue(
  value: unknown,
  valueType: ValueTypeName
): string {
  if (!value) return EMPTY_VALUE_LABEL;
  if (valueType === 'reference') return `Reference: ${value}`;
  if (valueType === 'composite') {
    return Array.isArray(value) ? `List · ${items(value.length)}` : 'List';
  }
  // Immediate array held as a real array (MCP / API authored).
  if (Array.isArray(value)) return items(value.length);
  try {
    const parsed = JSON.parse(String(value));
    if (Array.isArray(parsed)) return items(parsed.length);
  } catch {
    // Not JSON — fall through.
  }
  return EMPTY_VALUE_LABEL;
}

export function describeObjectValue(
  value: unknown,
  valueType: ValueTypeName,
  legacyFieldCount = 0
): string {
  // Legacy dot-notation fields win: the object is spread across sibling rows.
  if (legacyFieldCount > 0) return fields(legacyFieldCount);
  if (!value) return EMPTY_VALUE_LABEL;
  if (valueType === 'reference') return `Reference: ${value}`;
  if (valueType === 'composite') {
    return isPlainObject(value)
      ? `Object · ${fields(Object.keys(value).length)}`
      : 'Object';
  }
  // Immediate object held as a real object (MCP / API authored).
  if (isPlainObject(value)) return fields(Object.keys(value).length);
  try {
    const parsed = JSON.parse(String(value));
    if (isPlainObject(parsed)) return fields(Object.keys(parsed).length);
  } catch {
    // Not JSON — fall through.
  }
  return EMPTY_VALUE_LABEL;
}

/**
 * A condition held as an immediate (an Object Model query `condition`) as the
 * sentence its canvas card shows — `name is Input · supplier` — rather than as
 * "3 fields". Also reads the JSON text of one being edited. Empty for anything
 * that is not a condition expression.
 */
export function describeConditionValue(
  value: unknown,
  valueType: ValueTypeName
): string {
  if (valueType !== 'immediate') return '';
  let condition = value;
  if (typeof value === 'string') {
    // Ordinary text is not worth a parse attempt on every render.
    if (!value.trimStart().startsWith('{')) return '';
    try {
      condition = JSON.parse(value);
    } catch {
      return '';
    }
  }
  // The same test the canvas card applies (isConditionExpression).
  if (!isConditionExpression(condition)) return '';
  return describeCondition(condition);
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}
