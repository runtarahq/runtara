import { describe, expect, it } from 'vitest';
import {
  isTruncatedPayload,
  formatPayloadForDisplay,
  resolvePayloadForCopy,
} from './truncated-payload';

describe('isTruncatedPayload', () => {
  it('returns true for valid truncated payload', () => {
    expect(
      isTruncatedPayload({
        _original_size: 26264,
        _preview: '{"key": "value"}',
        _truncated: true,
      })
    ).toBe(true);
  });

  it('returns false when _truncated is false', () => {
    expect(
      isTruncatedPayload({
        _original_size: 100,
        _preview: '{}',
        _truncated: false,
      })
    ).toBe(false);
  });

  it('returns false for regular objects', () => {
    expect(isTruncatedPayload({ key: 'value' })).toBe(false);
  });

  it('returns false for null/undefined', () => {
    expect(isTruncatedPayload(null)).toBe(false);
    expect(isTruncatedPayload(undefined)).toBe(false);
  });

  it('returns false for primitives', () => {
    expect(isTruncatedPayload('string')).toBe(false);
    expect(isTruncatedPayload(42)).toBe(false);
  });

  it('returns false when _preview is missing', () => {
    expect(isTruncatedPayload({ _original_size: 100, _truncated: true })).toBe(
      false
    );
  });

  it('returns false when _original_size is missing', () => {
    expect(isTruncatedPayload({ _preview: '{}', _truncated: true })).toBe(
      false
    );
  });
});

describe('formatPayloadForDisplay', () => {
  it('returns pretty-printed preview for truncated payload with valid JSON', () => {
    const result = formatPayloadForDisplay({
      _original_size: 26264,
      _preview: '{"key":"value"}',
      _truncated: true,
    });

    expect(result.truncated).toBe(true);
    expect(result.text).toBe(JSON.stringify({ key: 'value' }, null, 2));
    expect(result.originalSize).toBe(26264);
    expect(result.originalSizeFormatted).toBe('25.65 KB');
  });

  it('returns raw preview string when preview is not valid JSON', () => {
    const result = formatPayloadForDisplay({
      _original_size: 50000,
      _preview: '{"key": "val',
      _truncated: true,
    });

    expect(result.truncated).toBe(true);
    expect(result.text).toBe('{"key": "val');
    expect(result.originalSize).toBe(50000);
  });

  it('returns regular JSON.stringify for non-truncated payloads', () => {
    const data = { key: 'value', nested: { a: 1 } };
    const result = formatPayloadForDisplay(data);

    expect(result.truncated).toBe(false);
    expect(result.text).toBe(JSON.stringify(data, null, 2));
    expect(result.originalSize).toBeUndefined();
    expect(result.originalSizeFormatted).toBeUndefined();
  });
});

describe('resolvePayloadForCopy', () => {
  it('returns preview string for truncated payload', () => {
    const result = resolvePayloadForCopy({
      _original_size: 26264,
      _preview: '{"key":"value"}',
      _truncated: true,
    });

    expect(result).toBe('{"key":"value"}');
  });

  it('returns JSON.stringify for regular payload', () => {
    const data = { key: 'value' };
    const result = resolvePayloadForCopy(data);

    expect(result).toBe(JSON.stringify(data, null, 2));
  });
});
