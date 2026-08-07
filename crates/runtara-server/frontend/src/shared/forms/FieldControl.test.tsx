import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { FieldControl } from './FieldControl';
import { inferControlKind, optionKey } from './control-registry';
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

describe('optionKey', () => {
  it('keeps string values plain and encodes richer ones', () => {
    expect(optionKey('password')).toBe('password');
    expect(optionKey(2)).toBe('2');
    expect(optionKey(true)).toBe('true');
    expect(optionKey({ id: 1 })).toBe('{"id":1}');
  });

  it('collapses empty and nullish values to a sentinel', () => {
    expect(optionKey('')).toBe('__empty__');
    expect(optionKey(null)).toBe('__empty__');
    expect(optionKey(undefined)).toBe('__empty__');
  });
});

describe('enum option values', () => {
  const enumField = field({ enum: ['password', 'private_key'] });

  it('renders string enum values without JSON quoting', () => {
    const { container } = render(
      <form>
        <FieldControl
          id="auth-mode"
          field={enumField}
          value="password"
          disabled={false}
          onChange={vi.fn()}
        />
      </form>
    );

    const values = [...container.querySelectorAll('select option')].map(
      (option) => option.getAttribute('value')
    );
    expect(values).toContain('password');
    expect(values).not.toContain('"password"');

    const select = container.querySelector('select');
    expect(select?.value).toBe('password');
  });

  it('renders multi-select enum values without JSON quoting', () => {
    const { container } = render(
      <FieldControl
        id="auth-modes"
        field={field({ type: 'array', enum: ['password', 'private_key'] })}
        value={['password']}
        disabled={false}
        onChange={vi.fn()}
      />
    );

    const select = container.querySelector<HTMLSelectElement>('select');
    expect([...(select?.options ?? [])].map((option) => option.value)).toEqual([
      'password',
      'private_key',
    ]);
    expect(
      [...(select?.selectedOptions ?? [])].map((option) => option.value)
    ).toEqual(['password']);
  });

  it('round-trips the selected option back as its original value', () => {
    const onChange = vi.fn();
    const { container } = render(
      <FieldControl
        id="ports"
        field={field({ type: 'array', control: { kind: 'multi_select' } })}
        value={[]}
        disabled={false}
        options={[
          { value: 'password', label: 'Password' },
          { value: 2, label: 'Two' },
        ]}
        onChange={onChange}
      />
    );

    const select = container.querySelector<HTMLSelectElement>('select')!;
    select.options[0].selected = true;
    fireEvent.change(select);
    expect(onChange).toHaveBeenLastCalledWith(['password']);

    select.options[0].selected = false;
    select.options[1].selected = true;
    fireEvent.change(select);
    expect(onChange).toHaveBeenLastCalledWith([2]);
  });

  it('renders an enum containing an empty value without crashing', () => {
    expect(() =>
      render(
        <form>
          <FieldControl
            id="optional-mode"
            field={field({ enum: ['', 'password'] })}
            value=""
            disabled={false}
            onChange={vi.fn()}
          />
        </form>
      )
    ).not.toThrow();
  });
});
