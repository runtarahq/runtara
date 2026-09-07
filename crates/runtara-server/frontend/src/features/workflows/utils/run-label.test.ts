import { describe, expect, it } from 'vitest';
import { executionDisplayName, runLabelSchema } from './run-label';

describe('run labels', () => {
  it('validates literal boundaries and punctuation', () => {
    for (const value of [
      null,
      '',
      'A',
      '0',
      '--[1]/--',
      'Order/AZ09-1.2 (done) [x]',
      'x'.repeat(250),
      'x'.repeat(251),
      'x'.repeat(500),
    ]) {
      expect(
        runLabelSchema.safeParse({ valueType: 'immediate', value }).success
      ).toBe(true);
    }
    for (const value of [
      '-'.repeat(250) + 'A',
      'x_y',
      '   ',
      '---',
      './()[] -',
      '\u200b',
      'x\u200by',
      '\u00a0',
      '\ufeff',
      '\u200e',
      'x%y',
      'x\\y',
      'x\ny',
      '\tx',
      'é',
      '🙂',
      '<x>',
      123,
    ]) {
      expect(
        runLabelSchema.safeParse({ valueType: 'immediate', value }).success
      ).toBe(false);
    }
  });

  it('accepts references and templates without validating their syntax as literal labels', () => {
    for (const label of [
      { valueType: 'reference', value: 'data.order_id', default: 'Unknown' },
      { valueType: 'template', value: 'Order/{{ data.order_id }}' },
    ])
      expect(runLabelSchema.parse(label)).toEqual(label);
  });

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
