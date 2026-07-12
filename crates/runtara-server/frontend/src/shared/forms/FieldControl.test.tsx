import { describe, expect, it } from 'vitest';

import { inferControlKind } from './control-registry';
import type { FormField } from './types';

const field = (patch: Partial<FormField> = {}): FormField => ({
  type: 'string',
  ...patch,
});

describe('inferControlKind', () => {
  it('prioritizes explicit controls and secret masking', () => {
    expect(inferControlKind(field({ control: { kind: 'radio' } }))).toBe(
      'radio'
    );
    expect(inferControlKind(field({ secret: true }))).toBe('password');
    expect(inferControlKind(field({ secret: true, format: 'textarea' }))).toBe(
      'secret_textarea'
    );
  });

  it('infers controls from enum, format, and field type', () => {
    expect(inferControlKind(field({ enum: ['a', 'b'] }))).toBe('select');
    expect(inferControlKind(field({ format: 'date' }))).toBe('date');
    expect(inferControlKind(field({ type: 'boolean' }))).toBe('toggle');
    expect(inferControlKind(field({ type: 'array' }))).toBe('tags');
    expect(inferControlKind(field({ type: 'object' }))).toBe('key_value');
  });
});
