import { describe, expect, it } from 'vitest';

import type { ConnectionTypeDto } from '@/generated/RuntaraRuntimeApi';
import type { FormDefinition } from '@/shared/forms';

import {
  buildConnectionFormDefinition,
  buildConnectionParameterPatch,
  buildConnectionParameterValues,
} from './adapter';

const descriptor: FormDefinition = {
  schemaVersion: 1,
  allowUnknownFields: false,
  sections: [
    { id: 'configuration', label: 'Connection details' },
    { id: 'credentials', label: 'Credentials' },
  ],
  fields: {
    client_id: { type: 'string', required: true, section: 'configuration' },
    client_secret: {
      type: 'string',
      required: true,
      access: 'write',
      secret: true,
      control: { kind: 'password' },
      section: 'credentials',
      conditions: {
        required: {
          type: 'operation',
          op: 'EQ',
          arguments: [
            { valueType: 'reference', value: 'client_id' },
            { valueType: 'immediate', value: 'client' },
          ],
        },
      },
    },
    realm_id: { type: 'string', access: 'read', section: 'configuration' },
    one_time_number: {
      type: 'integer',
      access: 'write',
      secret: false,
      section: 'configuration',
    },
  },
};

const connectionType = {
  integrationId: 'quickbooks_online',
  displayName: 'QuickBooks Online',
  fieldBehaviors: {},
  formDefinition: descriptor,
} as unknown as ConnectionTypeDto & { formDefinition: FormDefinition };

describe('connection canonical form adapter', () => {
  it('makes write-only credentials optional for edits without weakening create', () => {
    expect(
      buildConnectionFormDefinition(connectionType, 'create').fields
        .client_secret.required
    ).toBe(true);
    expect(
      buildConnectionFormDefinition(connectionType, 'edit', {
        client_secret: { configured: true, clearable: false },
      }).fields.client_secret.required
    ).toBe(false);
    expect(
      buildConnectionFormDefinition(connectionType, 'edit', {
        client_secret: { configured: true, clearable: false },
      }).fields.client_secret.conditions?.required
    ).toBeUndefined();
    expect(
      buildConnectionFormDefinition(connectionType, 'edit').fields.client_secret
        .required
    ).toBe(true);
    expect(
      buildConnectionFormDefinition(
        connectionType,
        'edit',
        { client_secret: { configured: true, clearable: true } },
        new Set(['client_secret'])
      ).fields.client_secret.conditions?.required
    ).toBeDefined();
  });

  it('loads safe readable values and never invents secret values', () => {
    const definition = buildConnectionFormDefinition(connectionType, 'edit', {
      client_secret: { configured: true, clearable: false },
    });
    const values = buildConnectionParameterValues(
      definition,
      {
        title: 'Accounting',
        editProjection: {
          values: { client_id: 'client', realm_id: 'company-1' },
          secretState: {
            client_secret: { configured: true, clearable: false },
          },
          version: 'v1',
        },
      },
      'edit'
    );

    expect(values).toMatchObject({
      title: 'Accounting',
      client_id: 'client',
      realm_id: 'company-1',
      client_secret: '',
    });
  });

  it('emits mutually exclusive preserve, replace, and explicit-clear operations', () => {
    expect(
      buildConnectionParameterPatch(
        descriptor,
        { client_id: 'client', client_secret: '' },
        new Set(),
        []
      )
    ).toEqual({ set: {}, write: {}, clear: [] });
    expect(
      buildConnectionParameterPatch(
        descriptor,
        { client_id: 'changed', client_secret: 'replacement' },
        new Set(['client_id', 'client_secret']),
        []
      )
    ).toEqual({
      set: { client_id: 'changed' },
      write: { client_secret: 'replacement' },
      clear: [],
    });
    expect(
      buildConnectionParameterPatch(
        descriptor,
        { client_id: 'client', client_secret: '' },
        new Set(['client_secret']),
        ['client_secret']
      )
    ).toEqual({
      set: {},
      write: {},
      clear: ['client_secret'],
    });
  });

  it('does not persist displayed defaults unless the field was changed', () => {
    const withDefault: FormDefinition = {
      ...descriptor,
      fields: {
        ...descriptor.fields,
        environment: {
          type: 'string',
          default: 'sandbox',
          section: 'configuration',
        },
      },
    };

    expect(
      buildConnectionParameterPatch(
        withDefault,
        { environment: 'sandbox' },
        new Set(),
        []
      )
    ).toEqual({ set: {}, write: {}, clear: [] });
    expect(
      buildConnectionParameterPatch(
        withDefault,
        { environment: 'production' },
        new Set(['environment']),
        []
      )
    ).toEqual({
      set: { environment: 'production' },
      write: {},
      clear: [],
    });
  });

  it('emits typed non-secret write-only values through write operations', () => {
    expect(
      buildConnectionParameterPatch(
        descriptor,
        { one_time_number: 42 },
        new Set(['one_time_number']),
        []
      )
    ).toEqual({
      set: {},
      write: { one_time_number: 42 },
      clear: [],
    });
  });
});
