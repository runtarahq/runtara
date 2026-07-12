import { describe, expect, it } from 'vitest';

import type { FormDefinition } from '@/shared/forms';
import { buildConnectionUpdateInput } from './update-payload';

const definition: FormDefinition = {
  fields: {
    environment: {
      type: 'string',
      default: 'sandbox',
      required: true,
    },
    client_secret: {
      type: 'string',
      access: 'write',
      secret: true,
    },
  },
};

describe('buildConnectionUpdateInput', () => {
  it('keeps displayed defaults and configured secrets out of a title-only update', () => {
    expect(
      buildConnectionUpdateInput({
        id: 'connection-1',
        data: {
          title: 'Renamed',
          environment: 'sandbox',
          client_secret: '',
          rateLimitEnabled: false,
        },
        dirtyFieldNames: ['title'],
        clearSecrets: [],
        definition,
        projection: {
          values: {},
          secretState: {
            client_secret: { configured: true, clearable: false },
          },
          version: 'v7',
        },
      })
    ).toEqual({
      id: 'connection-1',
      version: 'v7',
      title: 'Renamed',
      parameterPatch: undefined,
      rateLimitConfig: undefined,
      isDefaultFileStorage: undefined,
      defaultFor: undefined,
    });
  });

  it('emits only explicitly changed parameter and clear operations', () => {
    expect(
      buildConnectionUpdateInput({
        id: 'connection-1',
        data: {
          title: 'Original',
          environment: 'production',
          client_secret: '',
        },
        dirtyFieldNames: ['environment', 'client_secret'],
        clearSecrets: ['client_secret'],
        definition,
        projection: { version: 'v8' },
      }).parameterPatch
    ).toEqual({
      set: { environment: 'production' },
      write: {},
      clear: ['client_secret'],
    });
  });
});
