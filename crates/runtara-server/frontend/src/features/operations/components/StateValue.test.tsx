import { render, screen } from '@testing-library/react';
import { describe, it, expect } from 'vitest';
import { StateValue } from './StateValue';
import { StatePanel } from './RunStateCard';

describe('published state presentation', () => {
  it('uses ordinary formatting without inferring a currency', () => {
    const { rerender } = render(
      <StateValue value={48200} field={{ format: 'currency' }} />
    );
    expect(screen.getByText('48,200')).toBeInTheDocument();
    rerender(
      <StateValue
        value={48200}
        display={{ decimals: 2, prefix: '$', suffix: ' total' }}
      />
    );
    expect(screen.getByText('$48,200.00 total')).toBeInTheDocument();
    rerender(<StateValue value="$48,200 supplied by workflow" />);
    expect(
      screen.getByText('$48,200 supplied by workflow')
    ).toBeInTheDocument();
  });

  it('keeps false and zero distinct from missing state', () => {
    render(
      <StatePanel
        state={{ count: 0, accepted: false }}
        schema={{ count: { label: 'Items' }, missing: { label: 'Due' } }}
      />
    );
    expect(screen.getByText('Items')).toBeInTheDocument();
    expect(screen.getByText('0')).toBeInTheDocument();
    expect(screen.getByText('No')).toBeInTheDocument();
    expect(screen.getByLabelText('No value')).toBeInTheDocument();
  });

  it('renders complex state and untrusted strings as text', () => {
    render(
      <StatePanel
        state={{
          checks: [{ passed: true }],
          note: '<script>alert(1)</script>',
        }}
      />
    );
    expect(screen.getByText('[{"passed":true}]')).toBeInTheDocument();
    expect(screen.getByText('<script>alert(1)</script>')).toBeInTheDocument();
    expect(document.querySelector('script')).toBeNull();
  });

  it('provides the exact date on hover and preserves malformed values', () => {
    const { rerender } = render(
      <StateValue value="2026-09-29T10:00:00Z" field={{ format: 'datetime' }} />
    );
    expect(screen.getByTitle('2026-09-29T10:00:00Z')).toHaveAttribute(
      'datetime',
      '2026-09-29T10:00:00Z'
    );
    rerender(<StateValue value="not a date" field={{ format: 'datetime' }} />);
    expect(screen.getByText('not a date')).toBeInTheDocument();
  });
});
