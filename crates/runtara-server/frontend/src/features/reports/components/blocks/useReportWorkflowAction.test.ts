import { describe, expect, it } from 'vitest';
import type { ReportWorkflowActionConfig } from '../../types';
import { resolveWorkflowActionContext } from './useReportWorkflowAction';

describe('resolveWorkflowActionContext', () => {
  it('passes selected rows for table-wide workflow actions', () => {
    const action: ReportWorkflowActionConfig = {
      workflowId: 'process-items',
      context: { mode: 'selection', inputKey: 'items' },
    };
    const selectedRows = [
      { id: 'row-1', status: 'ready' },
      { id: 'row-2', status: 'blocked' },
    ];

    expect(
      resolveWorkflowActionContext(
        action,
        { id: 'ignored-row' },
        'ignored-value',
        'ignored_field',
        selectedRows
      )
    ).toEqual({ items: selectedRows });
  });
});
