import { describe, expect, it } from 'vitest';

import {
  buildSchemaFromFields,
  inferSchemaFromMapping,
  parseSchema,
} from './schema';

describe('inferSchemaFromMapping', () => {
  it('infers composite array outputs from array type hints', () => {
    expect(
      inferSchemaFromMapping([
        {
          type: 'items',
          typeHint: 'array',
          valueType: 'composite',
        },
      ])
    ).toEqual([
      {
        name: 'items',
        type: 'array',
        required: true,
      },
    ]);
  });
});

describe('parseSchema/buildSchemaFromFields', () => {
  it('round-trips rich DSL schema metadata', () => {
    const rawSchema = {
      items: {
        type: 'array',
        required: true,
        description: 'Order line items',
        default: [],
        example: [{ sku: 'sku_1', quantity: 2 }],
        items: {
          type: 'object',
          properties: {
            sku: { type: 'string', required: true },
            quantity: { type: 'integer', min: 1 },
          },
        },
        enum: [['sku_1'], ['sku_2']],
        label: 'Items',
        placeholder: '[]',
        order: 2,
        format: 'json',
        min: 1,
        max: 50,
        pattern: '^.+$',
        visibleWhen: { field: 'mode', equals: 'manual' },
        'x-runtime': { source: 'fixture' },
      },
    };

    const fields = parseSchema(rawSchema);
    expect(fields[0]).toMatchObject({
      name: 'items',
      type: 'array',
      required: true,
      example: [{ sku: 'sku_1', quantity: 2 }],
      enum: [['sku_1'], ['sku_2']],
      label: 'Items',
      placeholder: '[]',
      order: 2,
      format: 'json',
      min: 1,
      max: 50,
      pattern: '^.+$',
      visibleWhen: { field: 'mode', equals: 'manual' },
      extensions: { 'x-runtime': { source: 'fixture' } },
    });

    expect(buildSchemaFromFields(fields)).toEqual(rawSchema);
  });
});
