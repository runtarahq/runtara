import { describe, expect, it } from 'vitest';
import {
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
  });

  it('groups Split Scope suggestions under their own section', () => {
    const grouped = groupSuggestions(
      composeVariableSuggestions([], undefined, undefined, true, true)
    );

    expect(grouped['Split Scope']).toHaveLength(4);
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
    expect(waitScope.map((s) => s.value)).toEqual(['variables._signal_id']);
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

    expect(grouped['Wait Scope']).toHaveLength(1);
  });
});
