import { fireEvent, render, screen } from '@testing-library/react';
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

/**
 * An immediate holding a structure (authored via MCP or the REST API) is one
 * literal. Read as a Build, its bare keys rendered as "Unknown type".
 */
describe('ObjectMappingEditor — immediate structure', () => {
  const condition = {
    type: 'operation',
    op: 'EQ',
    arguments: [{ valueType: 'reference', value: 'name' }, 'acme'],
  };

  it('shows it as JSON in Value mode on an any-typed field', () => {
    renderEditor({ untyped: true, value: condition, valueType: 'immediate' });
    const box = screen.getByPlaceholderText(
      'Enter a JSON value...'
    ) as HTMLTextAreaElement;
    expect(box.value).toBe(JSON.stringify(condition, null, 2));
    expect(screen.queryByText('Unknown type')).toBeNull();
    expect(screen.queryByText('Composite Object')).toBeNull();
  });

  it('offers Value on a typed object field while it holds one', () => {
    renderEditor({ untyped: false, value: { a: 1 }, valueType: 'immediate' });
    expect(screen.getByText('Value')).toBeTruthy();
    expect(screen.getByPlaceholderText('Enter a JSON value...')).toBeTruthy();
  });

  it('still opens the empty seed of a typed object field in Build', () => {
    // New steps seed object inputs as an empty immediate with the json hint.
    renderEditor({
      untyped: false,
      value: '',
      valueType: 'immediate',
      typeHint: 'json',
    });
    expect(screen.queryByText('Value')).toBeNull();
    expect(screen.queryByPlaceholderText('Enter a JSON value...')).toBeNull();
  });

  it('edits it as text under the json hint, keeping it an immediate', () => {
    const onChange = vi.fn();
    const onValueTypeChange = vi.fn();
    const onTypeHintChange = vi.fn();
    renderEditor({
      untyped: true,
      value: condition,
      valueType: 'immediate',
      typeHint: 'auto',
      onChange,
      onValueTypeChange,
      onTypeHintChange,
    });
    fireEvent.change(screen.getByPlaceholderText('Enter a JSON value...'), {
      target: { value: '{"type": "operation"}' },
    });
    expect(onTypeHintChange).toHaveBeenCalledWith('json');
    expect(onChange).toHaveBeenCalledWith('{"type": "operation"}');
    expect(onValueTypeChange).not.toHaveBeenCalled();
  });

  it('ignores a click on the lit mode instead of resetting the value', () => {
    const onChange = vi.fn();
    const onValueTypeChange = vi.fn();
    renderEditor({
      untyped: true,
      value: condition,
      valueType: 'immediate',
      onChange,
      onValueTypeChange,
    });
    fireEvent.click(screen.getByText('Value'));
    expect(onChange).not.toHaveBeenCalled();
    expect(onValueTypeChange).not.toHaveBeenCalled();
  });

  it('drops the json hint it set when the mode changes', () => {
    const onTypeHintChange = vi.fn();
    renderEditor({
      untyped: true,
      value: '{"type":',
      valueType: 'immediate',
      typeHint: 'json',
      onTypeHintChange,
    });
    fireEvent.click(screen.getByText('Reference'));
    expect(onTypeHintChange).toHaveBeenCalledWith(undefined);
  });
});
