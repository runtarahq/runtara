import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { ConnectionConflictNotice } from './ConnectionConflictNotice';

describe('ConnectionConflictNotice', () => {
  it('keeps recovery actions disabled until the latest version is loaded', () => {
    render(
      <ConnectionConflictNotice
        message="Connection changed since it was opened"
        loadingLatest
        changedFields={[]}
        canRecover={false}
        applying={false}
        onReload={vi.fn()}
        onReapply={vi.fn()}
      />
    );
    expect(screen.getByText(/Loading the latest version/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Reload latest' })).toBeDisabled();
    expect(
      screen.getByRole('button', { name: 'Apply my submitted changes' })
    ).toBeDisabled();
  });

  it('shows server changes and requires an explicit reload or reapply action', () => {
    const onReload = vi.fn();
    const onReapply = vi.fn();
    render(
      <ConnectionConflictNotice
        message="Review the latest version before saving"
        loadingLatest={false}
        changedFields={['Title', 'environment']}
        canRecover
        applying={false}
        onReload={onReload}
        onReapply={onReapply}
      />
    );
    expect(
      screen.getByText(/Changed on the server: Title, environment/)
    ).toBeInTheDocument();
    expect(screen.getByText(/draft is still in the form/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Reload latest' }));
    fireEvent.click(
      screen.getByRole('button', { name: 'Apply my submitted changes' })
    );
    expect(onReload).toHaveBeenCalledTimes(1);
    expect(onReapply).toHaveBeenCalledTimes(1);
  });
});
