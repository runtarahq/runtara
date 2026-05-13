import { describe, expect, it } from 'vitest';

import {
  getFinishOutputValidationIssues,
  getFinishOutputValidationMessages,
} from './finish-output-validation';

describe('Finish output validation', () => {
  it('reports a missing source for a named Finish output row', () => {
    expect(
      getFinishOutputValidationIssues({
        stepType: 'Finish',
        inputMapping: [
          {
            type: 'orderId',
            value: '',
            valueType: 'reference',
          },
        ],
      })
    ).toEqual([
      {
        index: 0,
        field: 'value',
        path: ['inputMapping', 0, 'value'],
        message: 'Source is required',
      },
    ]);
  });

  it('reports a missing output name for a sourced Finish output row', () => {
    expect(
      getFinishOutputValidationIssues({
        stepType: 'Finish',
        inputMapping: [
          {
            type: '   ',
            value: 'steps.fetch.outputs.orderId',
            valueType: 'reference',
          },
        ],
      })
    ).toEqual([
      {
        index: 0,
        field: 'type',
        path: ['inputMapping', 0, 'type'],
        message: 'Output name is required',
      },
    ]);
  });

  it('returns workflow validation messages before graph composition can drop incomplete rows', () => {
    const messages = getFinishOutputValidationMessages([
      {
        id: 'finish',
        type: 'basic',
        position: { x: 0, y: 0 },
        data: {
          id: 'finish',
          name: 'Finish',
          stepType: 'Finish',
          inputMapping: [
            {
              type: 'orderId',
              value: '',
              valueType: 'immediate',
            },
          ],
        },
      },
    ] as any);

    expect(messages).toEqual([
      expect.objectContaining({
        severity: 'error',
        code: 'E_FINISH_OUTPUT_REQUIRED',
        message: 'Finish output row 1: Source is required',
        stepId: 'finish',
        stepName: 'Finish',
        fieldName: 'value',
        source: 'client',
      }),
    ]);
  });
});
