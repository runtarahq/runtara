import { describe, it, expect } from 'vitest';
import { describeFirstFormError, humanizeFieldPath } from './form-errors';

describe('humanizeFieldPath', () => {
  it('spaces camelCase', () => {
    expect(humanizeFieldPath('executionTimeout')).toBe('Execution timeout');
    expect(humanizeFieldPath('maxRetries')).toBe('Max retries');
  });

  it('renders array indices 1-based', () => {
    expect(humanizeFieldPath('inputMapping.2.value')).toBe(
      'Input mapping — item 3, value'
    );
    expect(humanizeFieldPath('inputMapping.0.type')).toBe(
      'Input mapping — item 1, type'
    );
  });

  it('drops the RHF root marker', () => {
    expect(humanizeFieldPath('inputMapping.root')).toBe('Input mapping');
  });

  it('handles snake and kebab segments', () => {
    expect(humanizeFieldPath('schema_name')).toBe('Schema name');
    expect(humanizeFieldPath('on-conflict')).toBe('On conflict');
  });

  it('passes an empty path through', () => {
    expect(humanizeFieldPath('')).toBe('');
  });
});

describe('describeFirstFormError', () => {
  it('returns null when there are no errors', () => {
    expect(describeFirstFormError({})).toBeNull();
    expect(describeFirstFormError(undefined)).toBeNull();
    expect(describeFirstFormError(null)).toBeNull();
  });

  it('finds a top-level field error', () => {
    const found = describeFirstFormError({
      name: { message: 'Name is required', type: 'too_small' },
    });
    expect(found).toEqual({
      path: 'name',
      message: 'Name is required',
      label: 'Name',
    });
  });

  it('finds an error nested in a field array with explicit undefined holes', () => {
    const found = describeFirstFormError({
      inputMapping: [
        undefined,
        undefined,
        {
          value: {
            message: 'Invalid JSON — Unexpected token } in JSON at position 8',
          },
        },
      ],
    });
    expect(found?.path).toBe('inputMapping.2.value');
    expect(found?.message).toContain('position 8');
    expect(found?.label).toBe('Input mapping — item 3, value');
  });

  it('finds an error in a GENUINELY sparse field array', () => {
    // This is the shape react-hook-form actually produces: the passing rows
    // are holes, not explicit undefined. `for…of` over a mapped sparse array
    // visits holes as undefined and throws on destructuring, so a naive walk
    // blows up inside onInvalid and the submit silently does nothing — the
    // exact failure this helper exists to prevent.
    const sparse: unknown[] = [];
    sparse[6] = { value: { message: 'Invalid JSON — Unexpected token }' } };
    const errors = { inputMapping: sparse };

    expect(() => describeFirstFormError(errors)).not.toThrow();
    const found = describeFirstFormError(errors);
    expect(found?.path).toBe('inputMapping.6.value');
    expect(found?.label).toBe('Input mapping — item 7, value');
  });

  it('returns null for a sparse array with no errors', () => {
    const sparse: unknown[] = [];
    sparse.length = 5;
    expect(describeFirstFormError({ inputMapping: sparse })).toBeNull();
  });

  it('skips RHF bookkeeping keys', () => {
    const found = describeFirstFormError({
      name: {
        ref: { name: 'name' },
        types: { required: 'x' },
        message: 'Name is required',
      },
    });
    expect(found?.path).toBe('name');
    expect(found?.message).toBe('Name is required');
  });

  it('ignores nodes with an empty or non-string message', () => {
    expect(describeFirstFormError({ a: { message: '' } })).toBeNull();
    expect(describeFirstFormError({ a: { message: 42 } })).toBeNull();
  });

  it('reports the first error in key order when several fail', () => {
    const found = describeFirstFormError({
      name: { message: 'first' },
      executionTimeout: { message: 'second' },
    });
    expect(found?.message).toBe('first');
  });

  it('finds an array-level root error', () => {
    const found = describeFirstFormError({
      inputMapping: { root: { message: 'At least one mapping is required' } },
    });
    expect(found?.path).toBe('inputMapping.root');
    expect(found?.label).toBe('Input mapping');
  });

  it('prefers a leaf over deeper nesting under the same node', () => {
    const found = describeFirstFormError({
      config: { message: 'Config is invalid', nested: { message: 'deeper' } },
    });
    expect(found?.path).toBe('config');
    expect(found?.message).toBe('Config is invalid');
  });
});
