import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { FilterComposer } from './index';

describe('FilterComposer — Add Condition inside AND group (SYN-215)', () => {
  const schemaDefinition = {
    name: { name: 'name', dataType: 'STRING' },
    age: { name: 'age', dataType: 'INTEGER' },
  };

  it('appends a full nested condition when clicking Add Condition in an AND group', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();

    // Start with an AND condition that already has one nested EQ child.
    const initialValue = {
      op: 'AND',
      arguments: [{ op: 'EQ', arguments: ['', ''] }],
    };

    render(
      <FilterComposer
        value={initialValue}
        onChange={onChange}
        schemaDefinition={schemaDefinition}
      />
    );

    // There should initially be one nested EQ condition → one "Select field..."
    const initialFieldSelectors = screen.getAllByText('Select field...');
    expect(initialFieldSelectors).toHaveLength(1);

    // Click "Add Condition" inside the AND group
    await user.click(screen.getByRole('button', { name: /add condition/i }));

    // After clicking, the onChange callback should have been called with an AND
    // condition that has TWO nested conditions, not a mix of condition + string.
    expect(onChange).toHaveBeenCalled();
    const lastCall = onChange.mock.calls[onChange.mock.calls.length - 1][0];
    expect(lastCall.op).toBe('AND');
    expect(lastCall.arguments).toHaveLength(2);
    // Both arguments should be condition objects with `op` property
    expect(lastCall.arguments[0]).toHaveProperty('op');
    expect(lastCall.arguments[1]).toHaveProperty('op');
  });
});
