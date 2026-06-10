import { describe, expect, it } from 'vitest';
import {
  buildCronConfiguration,
  buildWebhookConfiguration,
  isAcceptedCronExpression,
  parseStaticInputs,
  staticInputsError,
} from './trigger-configuration';

describe('staticInputsError', () => {
  it('returns null for blank or non-string values', () => {
    expect(staticInputsError('')).toBeNull();
    expect(staticInputsError('   ')).toBeNull();
    expect(staticInputsError(undefined)).toBeNull();
    expect(staticInputsError(null)).toBeNull();
  });

  it('returns null for a valid JSON object', () => {
    expect(staticInputsError('{}')).toBeNull();
    expect(
      staticInputsError('{"data": {"x": 1}, "variables": {"y": "z"}}')
    ).toBeNull();
  });

  it('flags invalid JSON', () => {
    expect(staticInputsError('{not json')).toBe(
      'Static inputs must be valid JSON.'
    );
    expect(staticInputsError('{"data": }')).toBe(
      'Static inputs must be valid JSON.'
    );
  });

  it('flags valid JSON that is not an object', () => {
    const objectError =
      'Static inputs must be a JSON object, e.g. {"data": {...}, "variables": {...}}.';
    expect(staticInputsError('[1, 2]')).toBe(objectError);
    expect(staticInputsError('"text"')).toBe(objectError);
    expect(staticInputsError('42')).toBe(objectError);
    expect(staticInputsError('null')).toBe(objectError);
  });
});

describe('parseStaticInputs', () => {
  it('returns undefined for blank or non-string values', () => {
    expect(parseStaticInputs('')).toBeUndefined();
    expect(parseStaticInputs('  \n ')).toBeUndefined();
    expect(parseStaticInputs(undefined)).toBeUndefined();
  });

  it('parses valid JSON', () => {
    expect(parseStaticInputs('{"data": {"a": 1}}')).toEqual({
      data: { a: 1 },
    });
  });

  it('throws on invalid JSON', () => {
    expect(() => parseStaticInputs('{nope')).toThrow();
  });
});

describe('buildCronConfiguration', () => {
  it('builds a minimal configuration with just the expression', () => {
    expect(buildCronConfiguration({ expression: '0 0 * * *' })).toEqual({
      expression: '0 0 * * *',
    });
  });

  it('includes parsed inputs and a real boolean debug flag', () => {
    expect(
      buildCronConfiguration({
        expression: '*/5 * * * *',
        inputsText: '{"data": {"region": "eu"}, "variables": {}}',
        debug: true,
      })
    ).toEqual({
      expression: '*/5 * * * *',
      inputs: { data: { region: 'eu' }, variables: {} },
      debug: true,
    });
  });

  it('preserves unknown keys from the existing configuration', () => {
    expect(
      buildCronConfiguration({
        existing: {
          expression: '0 0 * * *',
          timezone: 'Europe/Kyiv',
          custom_key: { nested: true },
        },
        expression: '0 12 * * *',
      })
    ).toEqual({
      expression: '0 12 * * *',
      timezone: 'Europe/Kyiv',
      custom_key: { nested: true },
    });
  });

  it('removes inputs and debug when cleared', () => {
    expect(
      buildCronConfiguration({
        existing: {
          expression: '0 0 * * *',
          inputs: { data: { a: 1 } },
          debug: true,
          other: 'kept',
        },
        expression: '0 0 * * *',
        inputsText: '',
        debug: false,
      })
    ).toEqual({
      expression: '0 0 * * *',
      other: 'kept',
    });
  });

  it('keeps the existing expression when no new one is provided', () => {
    expect(
      buildCronConfiguration({
        existing: { expression: '0 0 * * *' },
        inputsText: '{"data": {}}',
      })
    ).toEqual({
      expression: '0 0 * * *',
      inputs: { data: {} },
    });
  });

  it('does not mutate the existing configuration object', () => {
    const existing = {
      expression: '0 0 * * *',
      inputs: { data: {} },
      debug: true,
    };
    buildCronConfiguration({ existing, inputsText: '', debug: false });
    expect(existing).toEqual({
      expression: '0 0 * * *',
      inputs: { data: {} },
      debug: true,
    });
  });
});

describe('isAcceptedCronExpression', () => {
  it('accepts standard 5-field expressions', () => {
    expect(isAcceptedCronExpression('0 0 * * *')).toBe(true);
    expect(isAcceptedCronExpression('*/5 * * * *')).toBe(true);
    expect(isAcceptedCronExpression('  15 9 * * 1  ')).toBe(true);
  });

  it('accepts 6-field expressions whose seconds field is 0', () => {
    // Mirrors the server's normalize_cron_expression (cron_scheduler.rs),
    // which strips a leading '0' seconds field.
    expect(isAcceptedCronExpression('0 0 0 * * *')).toBe(true);
    expect(isAcceptedCronExpression('0 */5 * * * 1')).toBe(true);
  });

  it('rejects 6-field expressions with non-zero seconds', () => {
    expect(isAcceptedCronExpression('30 0 0 * * *')).toBe(false);
    expect(isAcceptedCronExpression('* 0 0 * * *')).toBe(false);
  });

  it('rejects other field counts and blank values', () => {
    expect(isAcceptedCronExpression('')).toBe(false);
    expect(isAcceptedCronExpression('   ')).toBe(false);
    expect(isAcceptedCronExpression('* * * *')).toBe(false);
    expect(isAcceptedCronExpression('0 0 0 0 * * *')).toBe(false);
    expect(isAcceptedCronExpression(undefined)).toBe(false);
    expect(isAcceptedCronExpression(null)).toBe(false);
  });
});

describe('buildWebhookConfiguration', () => {
  it('returns an empty object when nothing is set', () => {
    expect(buildWebhookConfiguration({})).toEqual({});
  });

  it('stores debug as a real boolean and connection_id as a string', () => {
    expect(
      buildWebhookConfiguration({ debug: true, connectionId: 'conn-1' })
    ).toEqual({
      debug: true,
      connection_id: 'conn-1',
    });
  });

  it('removes debug and connection_id when cleared', () => {
    expect(
      buildWebhookConfiguration({
        existing: { debug: true, connection_id: 'conn-1', other: 'kept' },
        debug: false,
        connectionId: '',
      })
    ).toEqual({ other: 'kept' });
  });

  it('preserves unknown keys from the existing configuration', () => {
    expect(
      buildWebhookConfiguration({
        existing: { custom_key: { nested: true }, path: '/hook' },
        debug: true,
        connectionId: 'conn-2',
      })
    ).toEqual({
      custom_key: { nested: true },
      path: '/hook',
      debug: true,
      connection_id: 'conn-2',
    });
  });

  it('does not mutate the existing configuration object', () => {
    const existing = { debug: true, connection_id: 'conn-1' };
    buildWebhookConfiguration({ existing, debug: false, connectionId: '' });
    expect(existing).toEqual({ debug: true, connection_id: 'conn-1' });
  });
});
