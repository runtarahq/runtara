import { describe, expect, it } from 'vitest';

import {
  controlValueToReportRange,
  reportEditorToFormField,
  reportFilterToFormField,
  reportRangeToControlValue,
} from './form-adapters';
import {
  canonicalConditionToReportVisibility,
  reportVisibilityToCanonicalCondition,
} from './utils';

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

  it('keeps a stable representative report-to-form normalization snapshot', () => {
    const visibility = {
      filter: 'status',
      equals: 'ready',
      notEquals: 'archived',
      exists: true,
    };
    const canonicalVisibility =
      reportVisibilityToCanonicalCondition(visibility);
    expect({
      filter: reportFilterToFormField(
        { id: 'status', label: 'Status', type: 'multi_select' },
        [{ label: 'Open', value: 'open', count: 3 }]
      ),
      editor: reportEditorToFormField('open', 'pill', { open: 'green' }, null),
      canonicalVisibility,
      persistedVisibility:
        canonicalConditionToReportVisibility(canonicalVisibility),
    }).toMatchInlineSnapshot(`
      {
        "canonicalVisibility": {
          "arguments": [
            {
              "arguments": [
                {
                  "type": "value",
                  "value": "status",
                  "valueType": "reference",
                },
                {
                  "type": "value",
                  "value": "ready",
                  "valueType": "immediate",
                },
              ],
              "op": "EQ",
              "type": "operation",
            },
            {
              "arguments": [
                {
                  "type": "value",
                  "value": "status",
                  "valueType": "reference",
                },
                {
                  "type": "value",
                  "value": "archived",
                  "valueType": "immediate",
                },
              ],
              "op": "NE",
              "type": "operation",
            },
            {
              "arguments": [
                {
                  "type": "value",
                  "value": "status",
                  "valueType": "reference",
                },
              ],
              "op": "IS_DEFINED",
              "type": "operation",
            },
          ],
          "op": "AND",
          "type": "operation",
        },
        "editor": {
          "control": {
            "kind": "select",
            "options": [
              {
                "label": "open",
                "value": "open",
              },
            ],
          },
          "max": undefined,
          "min": undefined,
          "pattern": undefined,
          "placeholder": undefined,
          "type": "string",
        },
        "filter": {
          "control": {
            "kind": "multi_select",
            "options": [
              {
                "label": "Open (3)",
                "value": "open",
              },
            ],
          },
          "label": "Status",
          "type": "array",
        },
        "persistedVisibility": {
          "equals": "ready",
          "exists": true,
          "filter": "status",
          "notEquals": "archived",
        },
      }
    `);
  });
});
