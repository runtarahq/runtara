import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { ConnectionFieldFrame } from './ConnectionFieldFrame';

describe('ConnectionFieldFrame', () => {
  it('offers explicit clear and undo only when descriptor state allows it', () => {
    const onClear = vi.fn();
    const onUndoClear = vi.fn();
    const { rerender } = render(
      <ConnectionFieldFrame
        label="Password"
        configured
        clearable
        cleared={false}
        onClear={onClear}
        onUndoClear={onUndoClear}
      />
    );

    fireEvent.click(
      screen.getByRole('button', { name: 'Clear stored Password' })
    );
    expect(onClear).toHaveBeenCalledOnce();

    rerender(
      <ConnectionFieldFrame
        label="Password"
        configured
        clearable
        cleared
        onClear={onClear}
        onUndoClear={onUndoClear}
      />
    );
    expect(
      screen.getByText('The stored secret will be cleared when you save.')
    ).toBeInTheDocument();
    fireEvent.click(
      screen.getByRole('button', { name: 'Undo clearing stored Password' })
    );
    expect(onUndoClear).toHaveBeenCalledOnce();
  });

  it('explains reauthorization and withholds forbidden clear actions', () => {
    render(
      <ConnectionFieldFrame
        label="Client Secret"
        configured
        clearable={false}
        cleared={false}
        requiresReauthorization
        onClear={vi.fn()}
        onUndoClear={vi.fn()}
      />
    );

    expect(screen.queryByRole('button')).not.toBeInTheDocument();
    expect(
      screen.getByText(
        'Replacing this value will require reconnecting the provider.'
      )
    ).toBeInTheDocument();
  });
});
