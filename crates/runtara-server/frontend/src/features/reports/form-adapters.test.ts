import { describe, expect, it } from 'vitest';

import {
  controlValueToReportRange,
  reportEditorToFormField,
  reportFilterToFormField,
  reportRangeToControlValue,
} from './form-adapters';

describe('report form adapters', () => {
  it('maps report filters to the shared control vocabulary', () => {
    expect(
      reportFilterToFormField(
        { id: 'status', label: 'Status', type: 'multi_select' },
        [{ label: 'Open', value: 'open' }]
      ).control
    ).toEqual({
      kind: 'multi_select',
      options: [{ label: 'Open', value: 'open' }],
    });
    expect(
      reportFilterToFormField({
        id: 'amount',
        label: 'Amount',
        type: 'number_range',
      }).control?.kind
    ).toBe('number_range');
  });

  it('adapts report range objects to shared range controls losslessly', () => {
    expect(
      controlValueToReportRange(reportRangeToControlValue({ min: 10, max: 20 }))
    ).toEqual({ min: 10, max: 20 });
  });

  it('maps inferred and explicit inline editors to shared fields', () => {
    expect(reportEditorToFormField(true, null, null, null).control?.kind).toBe(
      'toggle'
    );
    expect(
      reportEditorToFormField('open', 'pill', { open: 'green' }, null).control
    ).toEqual({ kind: 'select', options: [{ label: 'open', value: 'open' }] });
  });
});
