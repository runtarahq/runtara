import { describe, expect, it } from 'vitest';

import {
  isWorkflowActionDisabled,
  isWorkflowActionVisible,
  matchesReportRowCondition,
} from './utils';

describe('report row conditions', () => {
  const row = {
    id: 'row_1',
    status: 'ready',
    priority: 2,
    owner: { team: 'risk' },
    tags: ['urgent'],
  };

  it('matches scalar and nested row fields', () => {
    expect(
      matchesReportRowCondition(
        { op: 'EQ', arguments: ['status', 'ready'] },
        row
      )
    ).toBe(true);
    expect(
      matchesReportRowCondition(
        { op: 'EQ', arguments: ['owner.team', 'risk'] },
        row
      )
    ).toBe(true);
    expect(
      matchesReportRowCondition({ op: 'GT', arguments: ['priority', 1] }, row)
    ).toBe(true);
  });

  it('supports logical row conditions', () => {
    expect(
      matchesReportRowCondition(
        {
          op: 'AND',
          arguments: [
            { op: 'EQ', arguments: ['status', 'ready'] },
            { op: 'IN', arguments: ['priority', [1, 2, 3]] },
          ],
        },
        row
      )
    ).toBe(true);
    expect(
      matchesReportRowCondition(
        {
          op: 'NOT',
          arguments: [{ op: 'EQ', arguments: ['status', 'processed'] }],
        },
        row
      )
    ).toBe(true);
  });

  it('combines workflow action visibleWhen and hiddenWhen', () => {
    expect(
      isWorkflowActionVisible(
        {
          workflowId: 'workflow_1',
          visibleWhen: { op: 'EQ', arguments: ['status', 'ready'] },
          hiddenWhen: { op: 'EQ', arguments: ['priority', 5] },
        },
        row
      )
    ).toBe(true);
    expect(
      isWorkflowActionVisible(
        {
          workflowId: 'workflow_1',
          visibleWhen: { op: 'EQ', arguments: ['status', 'processed'] },
        },
        row
      )
    ).toBe(false);
    expect(
      isWorkflowActionVisible(
        {
          workflowId: 'workflow_1',
          hiddenWhen: { op: 'EQ', arguments: ['status', 'ready'] },
        },
        row
      )
    ).toBe(false);
  });

  it('evaluates workflow action disabledWhen independently from visibility', () => {
    const action = {
      workflowId: 'workflow_1',
      disabledWhen: { op: 'EQ', arguments: ['status', 'ready'] },
    };

    expect(isWorkflowActionVisible(action, row)).toBe(true);
    expect(isWorkflowActionDisabled(action, row)).toBe(true);
  });
});
