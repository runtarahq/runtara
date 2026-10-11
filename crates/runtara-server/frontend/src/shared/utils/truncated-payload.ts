interface TruncatedPayload {
  _original_size: number;
  _preview: string;
  _truncated: true;
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
 * An `_elided` stub the server emits for large input/output values (typically
 * inline base64 file uploads) on the default instance-detail fetch. The full
 * value is retrievable via the `?full=true` detail fetch.
 */
export interface ElidedStub {
  _elided: true;
  _original_size: number;
  _preview?: string;
}

export function isElidedStub(data: unknown): data is ElidedStub {
  if (typeof data !== 'object' || data === null) return false;
  const obj = data as Record<string, unknown>;
  return obj._elided === true && typeof obj._original_size === 'number';
}

/** True if any value anywhere in the tree is an elided stub. */
export function hasElidedPayload(data: unknown): boolean {
  if (isElidedStub(data)) return true;
  if (Array.isArray(data)) return data.some(hasElidedPayload);
  if (typeof data === 'object' && data !== null) {
    return Object.values(data as Record<string, unknown>).some(
      hasElidedPayload
    );
  }
  return false;
}
