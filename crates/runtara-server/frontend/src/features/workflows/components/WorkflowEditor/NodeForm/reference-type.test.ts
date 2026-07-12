import { describe, expect, it } from 'vitest';
import {
  describeStepReference,
  normalizeTypeName,
  parseStepReference,
  referenceTypeMismatch,
  resolveReferenceType,
} from './reference-type';
import type { StepInfo } from './shared';
import type { SchemaField } from '../EditorSidebar/SchemaFieldsEditor';
import type { SimpleVariable } from './NodeFormContext';

const PREVIOUS_STEPS: StepInfo[] = [
  {
    id: 'filt',
    name: 'Filter results',
    inputs: [],
    outputs: [
      {
        name: 'items',
        type: 'array',
        path: "steps['filt'].outputs.items",
      },
      {
        name: 'count',
        type: 'integer',
        path: "steps['filt'].outputs.count",
      },
    ],
  },
  {
    id: 'fetch',
    name: 'Fetch page',
    inputs: [],
    outputs: [
      {
        name: 'body',
        type: 'object',
        path: "steps['fetch'].outputs.body",
        children: [
          {
            name: 'body.token',
            type: 'string',
            path: "steps['fetch'].outputs.body.token",
          },
        ],
      },
    ],
  },
  {
    id: 'split',
    name: 'Split items',
    inputs: [],
    outputs: [
      { name: '', type: 'array', path: "steps['split'].outputs" },
      {
        name: 'hasFailures',
        type: 'boolean',
        path: "steps['split'].hasFailures",
      },
    ],
  },
];

const INPUT_SCHEMA: SchemaField[] = [
  { name: 'flag', type: 'string', required: true, description: '' },
  {
    name: 'customer',
    type: 'object',
    required: false,
    description: '',
    properties: [
      { name: 'email', type: 'string', required: false, description: '' },
    ],
  },
];

const VARIABLES: SimpleVariable[] = [
  { name: 'region', value: 'eu', type: 'String', description: null },
];

const CONTEXT = {
  previousSteps: PREVIOUS_STEPS,
  inputSchemaFields: INPUT_SCHEMA,
  variables: VARIABLES,
};

describe('parseStepReference', () => {
  it('parses bracket and dot spellings', () => {
    expect(parseStepReference("steps['filt'].outputs.items")).toEqual({
      stepId: 'filt',
      rest: 'outputs.items',
    });
    expect(parseStepReference('steps.filt.outputs.items')).toEqual({
      stepId: 'filt',
      rest: 'outputs.items',
    });
    expect(parseStepReference("steps['split'].hasFailures")).toEqual({
      stepId: 'split',
      rest: 'hasFailures',
    });
    expect(parseStepReference('data.flag')).toBeNull();
  });
});

describe('describeStepReference', () => {
  it('produces friendly labels for both spellings', () => {
    expect(
      describeStepReference("steps['filt'].outputs.items", PREVIOUS_STEPS)
    ).toEqual({ stepName: 'Filter results', fieldPath: 'items' });
    // Dot-form paths (hand-written or from imported graphs) used to render as
    // raw paths in the pill; they resolve the same way now.
    expect(
      describeStepReference('steps.filt.outputs.items', PREVIOUS_STEPS)
    ).toEqual({ stepName: 'Filter results', fieldPath: 'items' });
    expect(
      describeStepReference("steps['split'].hasFailures", PREVIOUS_STEPS)
    ).toEqual({ stepName: 'Split items', fieldPath: 'hasFailures' });
    expect(
      describeStepReference("steps['split'].outputs", PREVIOUS_STEPS)
    ).toEqual({ stepName: 'Split items', fieldPath: 'outputs' });
  });

  it('returns nothing for unknown steps or non-step paths', () => {
    expect(describeStepReference("steps['nope'].outputs", PREVIOUS_STEPS)).toEqual(
      {}
    );
    expect(describeStepReference('data.flag', PREVIOUS_STEPS)).toEqual({});
  });
});

describe('resolveReferenceType', () => {
  it('resolves step output fields in both spellings', () => {
    expect(resolveReferenceType("steps['filt'].outputs.items", CONTEXT)).toBe(
      'array'
    );
    expect(resolveReferenceType('steps.filt.outputs.count', CONTEXT)).toBe(
      'integer'
    );
  });

  it('resolves nested output children and sibling fields', () => {
    expect(
      resolveReferenceType("steps['fetch'].outputs.body.token", CONTEXT)
    ).toBe('string');
    expect(resolveReferenceType("steps['split'].hasFailures", CONTEXT)).toBe(
      'boolean'
    );
    expect(resolveReferenceType("steps['split'].outputs", CONTEXT)).toBe(
      'array'
    );
  });

  it('never guesses: unknown paths resolve to undefined', () => {
    expect(
      resolveReferenceType("steps['filt'].outputs.price", CONTEXT)
    ).toBeUndefined();
    expect(
      resolveReferenceType("steps['nope'].outputs", CONTEXT)
    ).toBeUndefined();
    expect(resolveReferenceType('item.email', CONTEXT)).toBeUndefined();
  });

  it('resolves workflow inputs, including nested properties', () => {
    expect(resolveReferenceType('data.flag', CONTEXT)).toBe('string');
    expect(resolveReferenceType('workflow.inputs.data.flag', CONTEXT)).toBe(
      'string'
    );
    expect(resolveReferenceType('data.customer.email', CONTEXT)).toBe(
      'string'
    );
    expect(resolveReferenceType('workflow.inputs.data', CONTEXT)).toBe(
      'object'
    );
    expect(resolveReferenceType('data.unknown', CONTEXT)).toBeUndefined();
  });

  it('resolves variables: user-declared and built-ins', () => {
    expect(resolveReferenceType('variables.region', CONTEXT)).toBe('string');
    expect(
      resolveReferenceType('workflow.inputs.variables.region', CONTEXT)
    ).toBe('string');
    expect(resolveReferenceType('variables._instance_id', CONTEXT)).toBe(
      'string'
    );
    expect(resolveReferenceType('variables._index', CONTEXT)).toBe('integer');
    expect(resolveReferenceType('variables._loop_indices', CONTEXT)).toBe(
      'array'
    );
    // _item is the runtime-shaped current Split item: honest unknown.
    expect(resolveReferenceType('variables._item', CONTEXT)).toBeUndefined();
    expect(resolveReferenceType('variables.unknown', CONTEXT)).toBeUndefined();
  });

  it('resolves loop context references', () => {
    expect(resolveReferenceType('loop.index', CONTEXT)).toBe('integer');
    expect(resolveReferenceType('loop.outputs', CONTEXT)).toBeUndefined();
  });

  it('treats data.* as unknown inside a Split body (rebound to the item)', () => {
    const insideSplit = { ...CONTEXT, insideSplitScope: true };
    expect(resolveReferenceType('data.flag', insideSplit)).toBeUndefined();
    expect(resolveReferenceType('data', insideSplit)).toBeUndefined();
    // Explicit workflow-scope spelling also refers to the rebound scope's
    // runtime resolution rules — stay honest and claim nothing.
    expect(
      resolveReferenceType('workflow.inputs.data.flag', insideSplit)
    ).toBeUndefined();
    // Step and variable references are unaffected by the data rebinding.
    expect(
      resolveReferenceType("steps['filt'].outputs.count", insideSplit)
    ).toBe('integer');
  });
});

describe('normalizeTypeName', () => {
  it('folds editor spellings to canonical JSON types', () => {
    expect(normalizeTypeName('text')).toBe('string');
    expect(normalizeTypeName('Int')).toBe('integer');
    expect(normalizeTypeName('double')).toBe('number');
    expect(normalizeTypeName('bool')).toBe('boolean');
    expect(normalizeTypeName('array<string>')).toBe('array');
    expect(normalizeTypeName('string[]')).toBe('array');
    expect(normalizeTypeName(undefined)).toBeUndefined();
  });
});

describe('referenceTypeMismatch', () => {
  it('is silent when either side is unknown or the target is a catch-all', () => {
    expect(referenceTypeMismatch(undefined, 'string')).toBeNull();
    expect(referenceTypeMismatch('array', undefined)).toBeNull();
    expect(referenceTypeMismatch('array', 'any')).toBeNull();
    expect(referenceTypeMismatch('any', 'string')).toBeNull();
    expect(referenceTypeMismatch('string', 'json')).toBeNull();
  });

  it('accepts identical and widening-compatible pairs', () => {
    expect(referenceTypeMismatch('string', 'text')).toBeNull();
    expect(referenceTypeMismatch('integer', 'number')).toBeNull();
    expect(referenceTypeMismatch('array', 'array<string>')).toBeNull();
  });

  it('warns on structural mismatches', () => {
    expect(referenceTypeMismatch('array', 'string')).toMatch(
      /Reference is array; this field expects string/
    );
    expect(referenceTypeMismatch('object', 'integer')).toMatch(/object/);
    expect(referenceTypeMismatch('number', 'integer')).toMatch(
      /expects integer/
    );
    expect(referenceTypeMismatch('boolean', 'string')).toMatch(/boolean/);
  });

  it('treats enum (select) fields as strings, not a distinct type', () => {
    // getInputComponentType returns 'select' for enum inputs; on the wire
    // they are strings — referencing a string into one must not warn.
    expect(referenceTypeMismatch('string', 'select')).toBeNull();
    expect(referenceTypeMismatch('array', 'select')).toMatch(/array/);
  });

  it('suppresses scalar→string warnings when the consumer coerces', () => {
    // Finish outputs with a "string" type hint always stringify scalars.
    const opts = { scalarsCoerceToString: true };
    expect(referenceTypeMismatch('integer', 'string', opts)).toBeNull();
    expect(referenceTypeMismatch('boolean', 'string', opts)).toBeNull();
    // Arrays/objects are not stringified by the hint — still a mismatch.
    expect(referenceTypeMismatch('array', 'string', opts)).toMatch(/array/);
    // Without the option the scalar warning stays (agent inputs don't coerce).
    expect(referenceTypeMismatch('integer', 'string')).toMatch(/integer/);
  });
});
