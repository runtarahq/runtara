import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { FieldControl } from './FieldControl';
import { inferControlKind } from './control-registry';
import type { FormControlKind, FormField } from './types';

const field = (patch: Partial<FormField> = {}): FormField => ({
  type: 'string',
  ...patch,
});

afterEach(cleanup);

describe('inferControlKind', () => {
  it('prioritizes explicit controls and secret masking', () => {
    expect(inferControlKind(field({ control: { kind: 'radio' } }))).toBe(
      'radio'
    );
    expect(inferControlKind(field({ secret: true }))).toBe('password');
    expect(inferControlKind(field({ secret: true, format: 'textarea' }))).toBe(
      'secret_textarea'
    );
  });

  it('infers controls from enum, format, and field type', () => {
    expect(inferControlKind(field({ enum: ['a', 'b'] }))).toBe('select');
    expect(inferControlKind(field({ format: 'date' }))).toBe('date');
    expect(inferControlKind(field({ type: 'boolean' }))).toBe('toggle');
    expect(inferControlKind(field({ type: 'array' }))).toBe('tags');
    expect(inferControlKind(field({ type: 'object' }))).toBe('key_value');
  });

  it('renders the fixture gallery for every canonical control kind', () => {
    const fixtures: Array<{
      kind: FormControlKind;
      type?: FormField['type'];
      value?: unknown;
      options?: Array<{ value: unknown; label: string }>;
    }> = [
      { kind: 'text' },
      { kind: 'textarea' },
      { kind: 'secret_textarea' },
      { kind: 'password' },
      { kind: 'number', type: 'number', value: 2 },
      { kind: 'toggle', type: 'boolean', value: true },
      { kind: 'select', options: [{ value: 'one', label: 'One' }] },
      {
        kind: 'multi_select',
        type: 'array',
        value: ['one'],
        options: [{ value: 'one', label: 'One' }],
      },
      { kind: 'radio', options: [{ value: 'one', label: 'One' }] },
      { kind: 'date' },
      { kind: 'datetime' },
      { kind: 'date_range', type: 'array', value: ['', ''] },
      { kind: 'number_range', type: 'array', value: [1, 2] },
      { kind: 'tags', type: 'array', value: ['one'] },
      { kind: 'key_value', type: 'object', value: { key: 'value' } },
      {
        kind: 'lookup',
        options: [{ value: 'one', label: 'One' }],
      },
      { kind: 'file', type: 'file' },
    ];

    for (const fixture of fixtures) {
      const labelId = `label-${fixture.kind}`;
      const { container, unmount } = render(
        <>
          <span id={labelId}>{fixture.kind}</span>
          <FieldControl
            id={`control-${fixture.kind}`}
            labelledBy={labelId}
            field={field({
              type: fixture.type ?? 'string',
              control: { kind: fixture.kind },
            })}
            value={fixture.value}
            disabled={false}
            options={fixture.options}
            onChange={vi.fn()}
          />
        </>
      );
      expect(
        container.querySelector(`#control-${fixture.kind}`),
        fixture.kind
      ).not.toBeNull();
      unmount();
    }

    render(
      <FieldControl
        id="accessible-key-value"
        labelledBy="accessible-key-value-label"
        field={field({ type: 'object', control: { kind: 'key_value' } })}
        value={{}}
        disabled={false}
        onChange={vi.fn()}
      />
    );
    const label = document.createElement('span');
    label.id = 'accessible-key-value-label';
    label.textContent = 'Accessible key value';
    document.body.append(label);
    expect(
      screen.getByRole('group', { name: 'Accessible key value' })
    ).toBeVisible();
    label.remove();
  });
});
