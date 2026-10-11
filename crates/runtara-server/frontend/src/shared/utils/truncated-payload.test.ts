import { describe, expect, it } from 'vitest';
import { isTruncatedPayload, hasElidedPayload } from './truncated-payload';

describe('elided payloads', () => {
  const elided = {
    _truncated: true,
    _elided: true,
    _original_size: 32 * 1024,
    _preview: 'AAAA',
  };

  it('detects an elided stub nested anywhere in the tree', () => {
    expect(hasElidedPayload({ data: { upload: { content: elided } } })).toBe(
      true
    );
    expect(hasElidedPayload([1, { x: elided }])).toBe(true);
    expect(hasElidedPayload({ data: { small: 'hi', n: 1 } })).toBe(false);
  });
});

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
