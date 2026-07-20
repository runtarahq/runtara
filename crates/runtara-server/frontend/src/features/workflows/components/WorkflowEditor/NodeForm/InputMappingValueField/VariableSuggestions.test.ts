import { describe, expect, it } from 'vitest';
import {
  composeConditionSuggestions,
  composeVariableSuggestions,
  groupSuggestions,
} from './VariableSuggestions';

describe('composeVariableSuggestions', () => {
  it('adds Split scope variables when inside a Split subgraph', () => {
    const suggestions = composeVariableSuggestions(
      [],
      undefined,
      undefined,
      false,
      true
    );

    const splitScope = suggestions.filter((s) => s.group === 'Split Scope');
    expect(splitScope.map((s) => s.value)).toEqual([
      'data',
      'variables._item',
      'variables._index',
      'variables._loop',
      'variables._loop_indices',
    ]);
  });

  it('omits Split scope variables outside a Split subgraph', () => {
    const suggestions = composeVariableSuggestions(
      [],
      undefined,
      undefined,
      false,
      false
    );

    expect(suggestions.some((s) => s.group === 'Split Scope')).toBe(false);
  });

  it('adds loop context entries when inside a While loop', () => {
    const suggestions = composeVariableSuggestions(
      [],
      undefined,
      undefined,
      true
    );

    const loopContext = suggestions.filter((s) => s.group === 'Loop Context');
    expect(loopContext.map((s) => s.value)).toEqual([
      'loop.index',
      'loop.outputs',
    ]);
    expect(
      suggestions
        .filter((s) => s.group === 'Iteration Context')
        .map((s) => s.value)
    ).toEqual(['iteration.index', 'iteration.indices', 'iteration.item']);
  });

  it('adds the same iteration context inside Split bodies', () => {
    const suggestions = composeVariableSuggestions(
      [],
      undefined,
      undefined,
      false,
      true
    );
    expect(
      suggestions
        .filter((s) => s.group === 'Iteration Context')
        .map((s) => s.value)
    ).toEqual(['iteration.index', 'iteration.indices', 'iteration.item']);
  });

  it('groups Split Scope suggestions under their own section', () => {
    const grouped = groupSuggestions(
      composeVariableSuggestions([], undefined, undefined, true, true)
    );

    // The four injected variables plus the rebound `data` (current item).
    expect(grouped['Split Scope']).toHaveLength(5);
    expect(grouped['Iteration Context']).toHaveLength(3);
    expect(grouped['Loop Context']).toHaveLength(2);
  });

  it('adds the signal id variable when inside a WaitForSignal onWait scope', () => {
    const suggestions = composeVariableSuggestions(
      [],
      undefined,
      undefined,
      false,
      false,
      true
    );

    const waitScope = suggestions.filter((s) => s.group === 'Wait Scope');
    // The onWait scope offers a bare `data` (its own undeclared schema) plus
    // the injected _signal_id — never the wrong-scope workflow inputs.
    expect(waitScope.map((s) => s.value)).toEqual([
      'data',
      'variables._signal_id',
    ]);
    expect(
      suggestions.some((s) => s.value.startsWith('workflow.inputs.data'))
    ).toBe(false);
  });

  it('omits Wait Scope variables outside an onWait scope', () => {
    const suggestions = composeVariableSuggestions(
      [],
      undefined,
      undefined,
      false,
      false,
      false
    );

    expect(suggestions.some((s) => s.group === 'Wait Scope')).toBe(false);
  });

  it('groups Wait Scope suggestions under their own section', () => {
    const grouped = groupSuggestions(
      composeVariableSuggestions([], undefined, undefined, false, false, true)
    );

    // Bare `data` (onWait scope) plus the injected _signal_id.
    expect(grouped['Wait Scope']).toHaveLength(2);
  });
});

describe('composeConditionSuggestions', () => {
  it('includes a single honest item entry for Filter conditions', () => {
    const suggestions = composeConditionSuggestions({
      previousSteps: [],
      includeItemScope: true,
    });

    const itemScope = suggestions.filter((s) => s.group === 'Current Item');
    expect(itemScope.map((s) => s.value)).toEqual(['item']);
    // The old forked composer invented item.id / item.name / item.title /
    // item.status … suggestions not driven by any schema — never again.
    expect(suggestions.some((s) => s.value.startsWith('item.'))).toBe(false);
  });

  it('omits the item scope unless requested', () => {
    const suggestions = composeConditionSuggestions({ previousSteps: [] });
    expect(suggestions.some((s) => s.group === 'Current Item')).toBe(false);
  });

  it('passes the canonical pipeline through (typed loop context)', () => {
    const suggestions = composeConditionSuggestions({
      previousSteps: [],
      isInsideWhileLoop: true,
    });

    const loopIndex = suggestions.find((s) => s.value === 'loop.index');
    expect(loopIndex?.group).toBe('Loop Context');
    expect(loopIndex?.type).toBe('number');
  });
});

describe('nested workflow input suggestions', () => {
  it('expands object properties into dotted, typed suggestions', () => {
    const suggestions = composeVariableSuggestions(
      [],
      [
        {
          name: 'customer',
          type: 'object',
          required: false,
          description: 'customer record',
          properties: [
            {
              name: 'email',
              type: 'string',
              required: false,
              description: '',
            },
            {
              name: 'address',
              type: 'object',
              required: false,
              description: '',
              properties: [
                {
                  name: 'city',
                  type: 'string',
                  required: false,
                  description: '',
                },
              ],
            },
          ],
        },
      ]
    );

    const byValue = Object.fromEntries(suggestions.map((s) => [s.value, s]));
    expect(byValue['workflow.inputs.data.customer']?.type).toBe('object');
    expect(byValue['workflow.inputs.data.customer.email']?.type).toBe('string');
    expect(byValue['workflow.inputs.data.customer.email']?.label).toBe(
      'customer.email'
    );
    // Two levels deep too.
    expect(byValue['workflow.inputs.data.customer.address.city']?.type).toBe(
      'string'
    );
  });
});

describe('Split item scope suggestions', () => {
  const ITEM_SCHEMA = [
    { name: 'sku', type: 'string', required: true, description: 'Item SKU' },
    {
      name: 'dims',
      type: 'object',
      required: false,
      description: '',
      properties: [
        { name: 'weight', type: 'number', required: false, description: '' },
      ],
    },
  ];

  it('suggests typed data.* from the declared iteration schema inside a Split', () => {
    const suggestions = composeVariableSuggestions(
      [],
      [{ name: 'flag', type: 'string', required: true, description: '' }],
      undefined,
      false,
      true, // inside Split
      false,
      ITEM_SCHEMA
    );

    const byValue = Object.fromEntries(suggestions.map((s) => [s.value, s]));
    expect(byValue['data']?.group).toBe('Split Scope');
    expect(byValue['data']?.type).toBe('object');
    expect(byValue['data.sku']?.type).toBe('string');
    expect(byValue['data.dims.weight']?.type).toBe('number');
    expect(byValue['iteration.item.sku']?.type).toBe('string');
    expect(byValue['iteration.item.dims.weight']?.type).toBe('number');
    // Workflow-level inputs are out of scope inside a Split body.
    expect(byValue['workflow.inputs.data.flag']).toBeUndefined();
    expect(byValue['workflow.inputs.data']).toBeUndefined();
  });

  it('offers an untyped data entry when the Split declares no schema', () => {
    const suggestions = composeVariableSuggestions(
      [],
      [{ name: 'flag', type: 'string', required: true, description: '' }],
      undefined,
      false,
      true,
      false,
      undefined
    );

    const dataEntry = suggestions.find((s) => s.value === 'data');
    expect(dataEntry?.group).toBe('Split Scope');
    expect(dataEntry?.type).toBeUndefined();
    expect(
      suggestions.some((s) => s.value.startsWith('workflow.inputs.data'))
    ).toBe(false);
  });

  it('keeps workflow inputs outside Split scopes', () => {
    const suggestions = composeVariableSuggestions(
      [],
      [{ name: 'flag', type: 'string', required: true, description: '' }],
      undefined,
      false,
      false,
      false,
      undefined
    );
    expect(
      suggestions.some((s) => s.value === 'workflow.inputs.data.flag')
    ).toBe(true);
  });
});
