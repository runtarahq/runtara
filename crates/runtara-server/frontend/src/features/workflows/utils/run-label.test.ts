import { describe, expect, it } from 'vitest';
import { executionDisplayName } from './run-label';

describe('run labels', () => {
  it('shows the label with existing fallbacks', () => {
    expect(
      executionDisplayName({
        runLabel: 'Order/12',
        workflowName: 'Orders',
        workflowId: 'id',
      })
    ).toBe('Order/12');
    expect(
      executionDisplayName({ runLabel: null, workflowName: 'Orders' })
    ).toBe('Orders');
    expect(executionDisplayName({ workflowId: 'id' })).toBe('id');
    expect(executionDisplayName({})).toBe('Ad-hoc invocation');
  });
});
