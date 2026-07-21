import { describe, expect, it } from 'vitest';

import { findModelResourceName } from './connection-resources';

describe('findModelResourceName', () => {
  it('uses the generic OpenAI model resource', () => {
    expect(
      findModelResourceName([
        { name: 'models', description: 'Available OpenAI models' },
      ])
    ).toBe('models');
  });

  it('uses an advertised namespaced model resource without inferring AWS', () => {
    expect(
      findModelResourceName([
        { name: 'sqs.queues' },
        { name: 'bedrock.models' },
      ])
    ).toBe('bedrock.models');
  });

  it('does not treat unrelated connection resources as AI models', () => {
    expect(findModelResourceName([{ name: 'sqs.queues' }])).toBeNull();
    expect(findModelResourceName(null)).toBeNull();
  });
});
