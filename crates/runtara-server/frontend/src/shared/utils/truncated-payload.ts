import { formatBytes } from '@/features/analytics/utils';

interface TruncatedPayload {
  _original_size: number;
  _preview: string;
  _truncated: true;
}

interface PayloadDisplayInfo {
  text: string;
  truncated: boolean;
  originalSize?: number;
  originalSizeFormatted?: string;
}

export function isTruncatedPayload(data: unknown): data is TruncatedPayload {
  if (typeof data !== 'object' || data === null) return false;
  const obj = data as Record<string, unknown>;
  return (
    obj._truncated === true &&
    typeof obj._preview === 'string' &&
    typeof obj._original_size === 'number'
  );
}

/**
 * Resolves a payload for display. If the payload is a truncated wrapper,
 * returns the preview content and truncation metadata. Otherwise returns
 * the JSON-stringified payload as-is.
 */
export function formatPayloadForDisplay(data: unknown): PayloadDisplayInfo {
  if (isTruncatedPayload(data)) {
    // Try to pretty-print the preview string if it's valid JSON
    let text = data._preview;
    try {
      text = JSON.stringify(JSON.parse(data._preview), null, 2);
    } catch {
      // Preview is not valid JSON (possibly truncated mid-string), use as-is
    }

    return {
      text,
      truncated: true,
      originalSize: data._original_size,
      originalSizeFormatted: formatBytes(data._original_size),
    };
  }

  return {
    text: JSON.stringify(data, null, 2),
    truncated: false,
  };
}

/**
 * Resolves a payload value for copying. If truncated, returns the preview content
 * rather than the wrapper object.
 */
export function resolvePayloadForCopy(data: unknown): string {
  if (isTruncatedPayload(data)) {
    return data._preview;
  }
  return JSON.stringify(data, null, 2);
}

/**
 * Resolves truncated payloads within a record's top-level values.
 * Returns a new object where any truncated payload values are replaced
 * with their parsed preview content, plus a `_truncation_info` field
 * summarizing which keys were truncated.
 */
export function resolveRecordPayloads(
  data: Record<string, unknown>
): Record<string, unknown> {
  const result: Record<string, unknown> = {};
  const truncatedKeys: string[] = [];

  for (const [key, value] of Object.entries(data)) {
    if (isTruncatedPayload(value)) {
      truncatedKeys.push(
        `${key} (original: ${formatBytes(value._original_size)})`
      );
      try {
        result[key] = JSON.parse(value._preview);
      } catch {
        result[key] = value._preview;
      }
    } else {
      result[key] = value;
    }
  }

  if (truncatedKeys.length > 0) {
    result._truncation_info = `Truncated fields: ${truncatedKeys.join(', ')}`;
  }

  return result;
}
