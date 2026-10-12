import { describe, expect, it } from 'vitest';
import { hasDirtyFields } from './dirty-state';

describe('hasDirtyFields', () => {
  it('is false for an empty or all-false tree', () => {
    expect(hasDirtyFields({})).toBe(false);
    expect(hasDirtyFields(undefined)).toBe(false);
    expect(
      hasDirtyFields({ inputMapping: [{ value: false }, undefined] })
    ).toBe(false);
  });

  it('finds a changed leaf at any depth', () => {
    expect(hasDirtyFields({ name: true })).toBe(true);
    expect(hasDirtyFields({ inputMapping: [undefined, { value: true }] })).toBe(
      true
    );
    expect(
      hasDirtyFields({ config: { cases: [{ output: { a: true } }] } })
    ).toBe(true);
  });
});
