import { describe, it, expect } from 'vitest';
import { canonicalAgentId, sameAgentId, findAgentById } from './agent-id';

describe('canonicalAgentId', () => {
  it('folds snake_case to kebab-case', () => {
    expect(canonicalAgentId('object_model')).toBe('object-model');
    expect(canonicalAgentId('ai_tools')).toBe('ai-tools');
    expect(canonicalAgentId('azure_blob_storage')).toBe('azure-blob-storage');
  });

  it('leaves an already-canonical id untouched', () => {
    expect(canonicalAgentId('object-model')).toBe('object-model');
    expect(canonicalAgentId('csv')).toBe('csv');
    expect(canonicalAgentId('s3-storage')).toBe('s3-storage');
  });

  it('lowercases stray capitalization', () => {
    expect(canonicalAgentId('Object_Model')).toBe('object-model');
    expect(canonicalAgentId('HTTP')).toBe('http');
  });

  it('is idempotent', () => {
    for (const id of ['object_model', 'Object_Model', 'csv', 's3-storage']) {
      const once = canonicalAgentId(id);
      expect(canonicalAgentId(once)).toBe(once);
    }
  });

  it('folds ASCII only, matching to_ascii_lowercase', () => {
    // Non-ASCII is passed through rather than Unicode-folded, so the result
    // never drifts from the Rust definition under a Turkish locale.
    expect(canonicalAgentId('İ')).toBe('İ');
    expect(canonicalAgentId('AÉ_B')).toBe('aÉ-b');
  });

  it('treats null/undefined/empty as no id', () => {
    expect(canonicalAgentId(null)).toBe('');
    expect(canonicalAgentId(undefined)).toBe('');
    expect(canonicalAgentId('')).toBe('');
  });
});

describe('sameAgentId', () => {
  it('matches across snake and kebab', () => {
    expect(sameAgentId('object_model', 'object-model')).toBe(true);
    expect(sameAgentId('object-model', 'object_model')).toBe(true);
    expect(sameAgentId('Object_Model', 'object-model')).toBe(true);
  });

  it('does not match different agents', () => {
    expect(sameAgentId('csv', 'xml')).toBe(false);
    expect(sameAgentId('object-model', 'object-modell')).toBe(false);
  });

  it('never matches when either side is missing', () => {
    expect(sameAgentId(null, null)).toBe(false);
    expect(sameAgentId('', '')).toBe(false);
    expect(sameAgentId('csv', undefined)).toBe(false);
    expect(sameAgentId(undefined, 'csv')).toBe(false);
  });
});

describe('findAgentById', () => {
  const agents = [
    { id: 'csv', name: 'CSV' },
    { id: 'object-model', name: 'Object Model' },
    { id: 's3-storage', name: 'S3' },
  ];

  it('finds a kebab catalog entry from a snake_case step agentId', () => {
    expect(findAgentById(agents, 'object_model')?.name).toBe('Object Model');
  });

  it('finds an exact match', () => {
    expect(findAgentById(agents, 'csv')?.name).toBe('CSV');
  });

  it('returns undefined for an unknown agent', () => {
    expect(findAgentById(agents, 'nope')).toBeUndefined();
  });

  it('returns undefined for missing inputs', () => {
    expect(findAgentById(agents, null)).toBeUndefined();
    expect(findAgentById(agents, '')).toBeUndefined();
    expect(findAgentById(null, 'csv')).toBeUndefined();
    expect(findAgentById(undefined, 'csv')).toBeUndefined();
  });

  it('folds the catalog side too, if a catalog ever serves snake', () => {
    const legacy = [{ id: 'object_model', name: 'Legacy' }];
    expect(findAgentById(legacy, 'object-model')?.name).toBe('Legacy');
  });
});
