import { describe, it, expect } from 'vitest';
import {
  resolveQuery,
  selectedFields,
  failureText,
  describeFailure,
} from './queries';
import { inlineOptions } from './answer-options';
import type { OperationViewConfig } from '@/generated/RuntaraRuntimeApi';

describe('Operations view queries', () => {
  const view: OperationViewConfig = {
    name: 'View',
    workflow: 'workflow',
    columns: ['amount', 'stage'],
    roles: { key: 'label', stage: 'stage', due: 'deadline' },
    where: {
      state: [
        {
          field: 'deadline',
          op: 'lt',
          value: { relative: 'now', offsetSeconds: -3600 },
        },
      ],
    },
    formats: { amount: { kind: 'number', prefix: '$', decimals: 2 } },
  };
  it('projects display roles once and leaves formatting out of the query', () => {
    expect(selectedFields(view)).toEqual([
      'amount',
      'stage',
      'label',
      'deadline',
    ]);
    expect(
      resolveQuery(view, Date.parse('2026-09-29T12:00:00Z'))
    ).not.toHaveProperty('formats');
  });
  it('resolves relative dates again for each refresh without mutating the saved view', () => {
    expect(
      resolveQuery(view, Date.parse('2026-09-29T12:00:00Z')).state?.[0].value
    ).toBe('2026-09-29T11:00:00.000Z');
    expect(
      resolveQuery(view, Date.parse('2026-09-29T13:00:00Z')).state?.[0].value
    ).toBe('2026-09-29T12:00:00.000Z');
    expect(view.where?.state?.[0].value).toEqual({
      relative: 'now',
      offsetSeconds: -3600,
    });
  });
  it('reads options from each registered schema and respects an explicitly disabled inline answer', () => {
    expect(
      inlineOptions({ decision: { enum: ['approve', 'reject'] } }, 'decision')
        ?.values
    ).toEqual(['approve', 'reject']);
    expect(
      inlineOptions(
        { type: 'object', properties: { decision: { enum: ['review'] } } },
        'decision'
      )?.values
    ).toEqual(['review']);
    expect(
      inlineOptions({ decision: { enum: ['approve'] } }, null, false)
    ).toBeNull();
    expect(
      inlineOptions({ answer: { enum: [true, false] } }, null, true)?.field
    ).toBe('answer');
  });
});

it('renders host failures as text and supplies structured error defaults', () => {
  expect(failureText({ message: 'Host exited' })).toBe('Host exited');
  expect(
    JSON.parse(
      failureText({ code: 'TEMP', category: 'transient', message: 'Retry' })
    )
  ).toMatchObject({ severity: 'error', attributes: {} });
});

it('shows structured terminal errors as messages and preserves plain host failures', () => {
  expect(
    describeFailure({
      error: JSON.stringify({
        message: 'Please retry',
        category: 'transient',
        code: 'TEMP',
      }),
    })
  ).toEqual({ message: 'Please retry', category: 'transient', code: 'TEMP' });
  expect(describeFailure({ error: 'Worker exited' })).toEqual({
    message: 'Worker exited',
  });
  expect(describeFailure({ error: '{malformed' })).toEqual({
    message: '{malformed',
  });
});
