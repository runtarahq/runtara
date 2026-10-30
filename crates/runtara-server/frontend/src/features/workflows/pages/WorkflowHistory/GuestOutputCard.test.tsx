import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { GuestOutputCard } from './GuestOutputCard';

describe('GuestOutputCard', () => {
  it('shows what the guest wrote', () => {
    render(<GuestOutputCard output={'fleet guest says hello\nsecond line'} />);
    expect(screen.getByText('Guest output')).toBeTruthy();
    expect(screen.getByTestId('guest-output').textContent).toContain(
      'fleet guest says hello\nsecond line'
    );
  });

  it('is hidden when the runner kept no capture', () => {
    const { container } = render(<GuestOutputCard output={null} />);
    expect(container.firstChild).toBeNull();
  });
});
