import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

import type { Instance } from '@/generated/RuntaraRuntimeApi';
import { EditableCell } from './EditableCell';

const ROW: Instance = {
  id: 'row-1',
  tenantId: 'tenant-1',
  createdAt: '2026-07-27T00:00:00Z',
  updatedAt: '2026-07-27T00:00:00Z',
  properties: { label: 'before' },
};

function renderCell() {
  const props = {
    getValue: () => 'before' as unknown,
    row: { original: ROW },
    column: { id: 'label' },
    onUpdate: vi.fn(),
    onCommitRow: vi.fn(),
    setIsEditing: vi.fn(),
    dataType: 'string' as const,
    isEditing: true,
  };

  render(<EditableCell {...props} />);
  return props;
}

describe('EditableCell', () => {
  it('writes the row and closes the editor on Enter', async () => {
    const user = userEvent.setup();
    const props = renderCell();

    const input = screen.getByRole('textbox');
    await user.clear(input);
    await user.type(input, 'after');
    await user.keyboard('{Enter}');

    // Enter used to only mark the row dirty, leaving the edit unsent and the
    // editor open with nothing on screen to say the value had not been saved.
    expect(props.onCommitRow).toHaveBeenCalled();
    expect(props.setIsEditing).toHaveBeenCalledWith(false);
    expect(props.onUpdate).toHaveBeenCalledWith(
      'row-1',
      expect.objectContaining({
        properties: expect.objectContaining({ label: 'after' }),
      })
    );
  });

  it('discards the edit on Escape without writing the row', async () => {
    const user = userEvent.setup();
    const props = renderCell();

    const input = screen.getByRole('textbox');
    await user.clear(input);
    await user.type(input, 'discard me');
    await user.keyboard('{Escape}');

    expect(props.onCommitRow).not.toHaveBeenCalled();
    expect(props.setIsEditing).toHaveBeenCalledWith(false);
  });
});
