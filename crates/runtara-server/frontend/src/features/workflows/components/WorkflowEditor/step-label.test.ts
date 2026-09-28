import { describe, expect, it } from 'vitest';
import { stepIdLabel } from './step-label';

describe('stepIdLabel', () => {
  it('labels unnamed steps by readable ids but not generated ones', () => {
    expect(stepIdLabel('repeat')).toBe('repeat');
    expect(stepIdLabel('3590e4e9-1c67-4fc9-8fc2-cbd182d17254')).toBeUndefined();
    expect(stepIdLabel(undefined)).toBeUndefined();
    expect(stepIdLabel('')).toBeUndefined();
  });
});
