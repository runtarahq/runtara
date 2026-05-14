import { describe, expect, it } from 'vitest';

import { inferSchemaFromMapping } from './schema';

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
