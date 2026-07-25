import { describe, it, expect } from 'vitest';
import {
  entriesToMappingJson,
  formatMappingJson,
  parseMappingJson,
} from './mapping-json';
import type { SeededMappingEntry } from './mapping-entries';

const entry = (e: Partial<SeededMappingEntry> & { type: string }) =>
  e as SeededMappingEntry;

describe('entriesToMappingJson', () => {
  it('keys by field name and keeps the value type', () => {
    expect(
      entriesToMappingJson([
        entry({
          type: 'schema_name',
          value: 'Product',
          valueType: 'immediate',
        }),
        entry({
          type: 'instances',
          value: 'steps.parse.outputs',
          valueType: 'reference',
        }),
      ])
    ).toEqual({
      schema_name: { value: 'Product', valueType: 'immediate' },
      instances: { value: 'steps.parse.outputs', valueType: 'reference' },
    });
  });

  it('omits an auto type hint but keeps a real one', () => {
    const out = entriesToMappingJson([
      entry({
        type: 'a',
        value: '1',
        valueType: 'immediate',
        typeHint: 'auto',
      }),
      entry({
        type: 'b',
        value: '1',
        valueType: 'immediate',
        typeHint: 'integer',
      }),
    ]);
    expect(out.a).not.toHaveProperty('typeHint');
    expect(out.b.typeHint).toBe('integer');
  });

  it('keeps a reference fallback', () => {
    const out = entriesToMappingJson([
      entry({
        type: 'a',
        value: 'steps.x.outputs.y',
        valueType: 'reference',
        defaultValue: 0,
      }),
    ]);
    expect(out.a.defaultValue).toBe(0);
  });

  it('defaults a missing valueType to immediate and skips unnamed entries', () => {
    const out = entriesToMappingJson([
      entry({ type: 'a', value: 'x' }),
      entry({ type: '', value: 'ignored' }),
    ]);
    expect(out.a.valueType).toBe('immediate');
    expect(Object.keys(out)).toEqual(['a']);
  });

  it('handles no entries', () => {
    expect(entriesToMappingJson(undefined)).toEqual({});
    expect(entriesToMappingJson([])).toEqual({});
  });
});

describe('parseMappingJson', () => {
  it('round-trips the formatted output losslessly', () => {
    const original = [
      entry({ type: 'schema_name', value: 'Product', valueType: 'immediate' }),
      entry({
        type: 'instances',
        value: 'steps.parse.outputs',
        valueType: 'reference',
        defaultValue: null,
      }),
      entry({
        type: 'conflict_columns',
        value: ['sku'],
        valueType: 'immediate',
        typeHint: 'json',
      }),
    ];
    const { entries, error } = parseMappingJson(formatMappingJson(original));
    expect(error).toBeNull();
    expect(entries).toEqual(original);
  });

  it('accepts the DSL spelling `default` for a fallback', () => {
    // This is what pasting straight out of MCP or the REST API looks like.
    const { entries, error } = parseMappingJson(
      '{"a": {"value": "steps.x.outputs.y", "valueType": "reference", "default": 7}}'
    );
    expect(error).toBeNull();
    expect(entries?.[0].defaultValue).toBe(7);
  });

  it('treats a bare literal as an immediate value', () => {
    const { entries, error } = parseMappingJson('{"limit": 25, "name": "abc"}');
    expect(error).toBeNull();
    expect(entries).toEqual([
      { type: 'limit', value: 25, valueType: 'immediate' },
      { type: 'name', value: 'abc', valueType: 'immediate' },
    ]);
  });

  it('preserves structural immediate values', () => {
    const { entries } = parseMappingJson(
      '{"rows": {"value": [[1,2],[3,4]], "valueType": "immediate"}}'
    );
    expect(entries?.[0].value).toEqual([
      [1, 2],
      [3, 4],
    ]);
  });

  it('treats blank input as an empty mapping', () => {
    expect(parseMappingJson('')).toEqual({ entries: [], error: null });
    expect(parseMappingJson('   \n ')).toEqual({ entries: [], error: null });
  });

  it('reports a positioned message for malformed JSON', () => {
    const { entries, error } = parseMappingJson('{"a": 1,}');
    expect(entries).toBeNull();
    expect(error).toMatch(/^Invalid JSON — /);
    expect(error).toMatch(/position/);
  });

  it('rejects a non-object document with guidance', () => {
    expect(parseMappingJson('[1,2,3]').error).toMatch(
      /keyed by parameter name/
    );
    expect(parseMappingJson('"nope"').error).toMatch(/keyed by parameter name/);
  });

  it('rejects an unknown valueType by name', () => {
    const { entries, error } = parseMappingJson(
      '{"a": {"value": "x", "valueType": "magic"}}'
    );
    expect(entries).toBeNull();
    expect(error).toContain('"a"');
    expect(error).toContain('magic');
  });

  it('accepts every real valueType', () => {
    for (const vt of ['immediate', 'reference', 'template', 'composite']) {
      const { error } = parseMappingJson(
        `{"a": {"value": "x", "valueType": "${vt}"}}`
      );
      expect(error).toBeNull();
    }
  });

  it('rejects an empty parameter name', () => {
    expect(parseMappingJson('{"": {"value": 1}}').error).toMatch(
      /cannot be empty/
    );
  });
});
