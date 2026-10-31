import { describe, expect, it } from 'vitest';
import { isCompatibleServer } from './serverCompat';

describe('isCompatibleServer', () => {
  it('accepts the same minor and one minor either way', () => {
    expect(isCompatibleServer('10.2.0', '10.2.7')).toBe(true);
    expect(isCompatibleServer('10.2.0', '10.1.3')).toBe(true);
    expect(isCompatibleServer('10.2.0', '10.3.0')).toBe(true);
  });

  it('rejects minors further apart and another major', () => {
    expect(isCompatibleServer('10.2.0', '10.0.0')).toBe(false);
    expect(isCompatibleServer('10.0.0', '9.9.0')).toBe(false);
  });

  it('says nothing about versions that are not releases', () => {
    expect(isCompatibleServer('dev', '10.2.0')).toBeNull();
    expect(isCompatibleServer('10.2.0', undefined)).toBeNull();
    expect(isCompatibleServer('v10.2.0', '10.2.0-rc.1')).toBe(true);
  });
});
