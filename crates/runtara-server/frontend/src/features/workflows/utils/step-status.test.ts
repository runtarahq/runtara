import { describe, expect, it } from 'vitest';
import { stepStatusDisplay } from './step-status';

describe('stepStatusDisplay', () => {
  it('renders a suspended step as its own non-running, non-failed state', () => {
    const suspended = stepStatusDisplay('suspended');
    expect(suspended).toMatchObject({
      label: 'Suspended',
      badgeVariant: 'warning',
      spin: false,
    });
    expect(suspended.badgeVariant).not.toBe(
      stepStatusDisplay('running').badgeVariant
    );
    expect(suspended.badgeVariant).not.toBe(
      stepStatusDisplay('failed').badgeVariant
    );
  });

  it('keeps the existing running/completed/failed mapping', () => {
    expect(stepStatusDisplay('running')).toMatchObject({
      label: 'Running',
      badgeVariant: 'secondary',
      spin: true,
    });
    expect(stepStatusDisplay('completed').badgeVariant).toBe('default');
    expect(stepStatusDisplay('failed').badgeVariant).toBe('destructive');
    expect(stepStatusDisplay('cancelled')).toMatchObject({
      label: 'cancelled',
      badgeVariant: 'outline',
    });
  });
});
