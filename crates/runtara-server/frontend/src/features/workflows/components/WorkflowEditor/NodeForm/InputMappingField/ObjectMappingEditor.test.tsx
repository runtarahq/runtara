import { render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { ObjectMappingEditor } from './ObjectMappingEditor';
import { NodeFormContext } from '../NodeFormContext';

/**
 * An `any`-typed capability input accepts anything the DSL can express, but
 * the editor used to fold it into the object-only path: reference-or-build,
 * seeded to `{}`, with the object/array switcher suppressed. There was no way
 * to set such a field to a list or to a plain scalar without leaving the UI.
 * 73 inputs across 110 of 305 capabilities are `any`.
 */
function renderEditor(
  props: Partial<Parameters<typeof ObjectMappingEditor>[0]>
) {
  const ctx = {
    previousSteps: [],
    inputSchemaFields: [],
    variables: [],
    isInsideSplit: false,
    isInsideWaitScope: false,
    splitItemSchemaFields: [],
    nodeId: 'step-1',
    agents: [],
    workflows: [],
    stepTypes: [],
  };
  return render(
    <NodeFormContext.Provider value={ctx as never}>
      <ObjectMappingEditor
        value={{}}
        valueType="composite"
        onChange={vi.fn()}
        onValueTypeChange={vi.fn()}
        onClose={vi.fn()}
        {...props}
      />
    </NodeFormContext.Provider>
  );
}

describe('ObjectMappingEditor — typed object field', () => {
  it('offers only Reference and Build', () => {
    renderEditor({ untyped: false });
    expect(screen.getByText('Reference')).toBeTruthy();
    expect(screen.getByText('Build')).toBeTruthy();
    expect(screen.queryByText('Value')).toBeNull();
  });

  it('does not offer an array root — a typed object field is an object', () => {
    renderEditor({ untyped: false });
    expect(screen.queryByText('Composite Array')).toBeNull();
  });
});

describe('ObjectMappingEditor — any-typed field', () => {
  it('offers Value, Reference and Build', () => {
    renderEditor({ untyped: true });
    expect(screen.getByText('Value')).toBeTruthy();
    expect(screen.getByText('Reference')).toBeTruthy();
    expect(screen.getByText('Build')).toBeTruthy();
  });

  it('exposes the object/array switcher so the root can become a list', () => {
    renderEditor({ untyped: true, value: {}, valueType: 'composite' });
    // The labels appear both in the switcher and in the editor heading, so
    // assert on presence rather than uniqueness.
    expect(screen.getAllByText('Composite Object').length).toBeGreaterThan(0);
    expect(screen.getAllByText('Composite Array').length).toBeGreaterThan(0);
  });

  it('starts in Value mode for a plain scalar', () => {
    // A non-structural immediate is a value, not an empty object.
    const { container } = renderEditor({
      untyped: true,
      value: 'hello' as never,
      valueType: 'immediate',
    });
    expect(
      container.querySelector('[placeholder="Enter a value..."]')
    ).toBeTruthy();
  });

  it('starts in Build mode when the value is already a structure', () => {
    renderEditor({
      untyped: true,
      value: { a: { valueType: 'immediate', value: 1 } } as never,
      valueType: 'composite',
    });
    expect(screen.getAllByText('Composite Object').length).toBeGreaterThan(0);
    // Not the Value editor — the existing structure is preserved, not reset.
    expect(
      document.querySelector('[placeholder="Enter a value..."]')
    ).toBeNull();
  });

  it('switching to Value clears to an immediate rather than seeding {}', () => {
    const onChange = vi.fn();
    const onValueTypeChange = vi.fn();
    renderEditor({
      untyped: true,
      value: {} as never,
      valueType: 'composite',
      onChange,
      onValueTypeChange,
    });
    screen.getByText('Value').click();
    expect(onValueTypeChange).toHaveBeenCalledWith('immediate');
    expect(onChange).toHaveBeenCalledWith('');
  });
});
