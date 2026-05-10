import { describe, expect, it } from 'vitest';
import type { Instance } from '@/generated/RuntaraRuntimeApi';
import type { ReportBlockResult, ReportRenderResponse } from '../../../types';
import { patchReportWritebackQueryData } from './reportWritebackCache';

describe('patchReportWritebackQueryData', () => {
  it('patches only the matching object row in a render response', () => {
    const targetRow = {
      id: 'row_1',
      schemaId: 'schema_1',
      status: 'open',
    };
    const otherRow = {
      id: 'row_2',
      schemaId: 'schema_1',
      status: 'open',
    };
    const metricBlock: ReportBlockResult = {
      type: 'metric',
      status: 'ready',
      data: { value: 2 },
    };
    const response: ReportRenderResponse = {
      success: true,
      report: { id: 'report_1', definitionVersion: 1 },
      resolvedFilters: {},
      blocks: {
        table_1: {
          type: 'table',
          status: 'ready',
          data: {
            columns: ['id', 'schemaId', 'status'],
            rows: [targetRow, otherRow],
          },
        },
        metric_1: metricBlock,
      },
      errors: [],
    };

    const patched = patchReportWritebackQueryData(response, {
      schemaId: 'schema_1',
      instanceId: 'row_1',
      field: 'status',
      value: 'closed',
    });

    expect(patched).not.toBe(response);
    expect(patched.blocks.metric_1).toBe(metricBlock);

    const patchedData = patched.blocks.table_1.data as {
      rows: Array<Record<string, unknown>>;
    };
    expect(patchedData.rows[0]).toEqual({
      id: 'row_1',
      schemaId: 'schema_1',
      status: 'closed',
    });
    expect(patchedData.rows[0]).not.toBe(targetRow);
    expect(patchedData.rows[1]).toBe(otherRow);
  });

  it('patches matching array rows using table columns', () => {
    const targetRow = ['row_1', 'schema_1', 'open'];
    const otherRow = ['row_2', 'schema_1', 'open'];
    const block: ReportBlockResult = {
      type: 'table',
      status: 'ready',
      data: {
        columns: ['id', 'schemaId', 'status'],
        rows: [targetRow, otherRow],
      },
    };

    const patched = patchReportWritebackQueryData(block, {
      schemaId: 'schema_1',
      instanceId: 'row_1',
      field: 'status',
      value: 'closed',
    });

    expect(patched).not.toBe(block);
    const patchedData = patched.data as { rows: unknown[][] };
    expect(patchedData.rows[0]).toEqual(['row_1', 'schema_1', 'closed']);
    expect(patchedData.rows[0]).not.toBe(targetRow);
    expect(patchedData.rows[1]).toBe(otherRow);
  });

  it('patches card rows from the server-returned instance', () => {
    const row = {
      id: 'row_1',
      schemaId: 'schema_1',
      status: 'open',
      updatedAt: '2026-01-01T00:00:00Z',
    };
    const block: ReportBlockResult = {
      type: 'card',
      status: 'ready',
      data: { row },
    };
    const instance: Instance = {
      id: 'row_1',
      schemaId: 'schema_1',
      schemaName: 'Case',
      tenantId: 'tenant_1',
      createdAt: '2026-01-01T00:00:00Z',
      updatedAt: '2026-01-02T00:00:00Z',
      properties: {
        status: 'closed',
        owner: 'Ada',
      },
    };

    const patched = patchReportWritebackQueryData(block, {
      schemaId: 'schema_1',
      instanceId: 'row_1',
      field: 'status',
      value: 'closed',
      instance,
    });

    expect(patched).not.toBe(block);
    const patchedData = patched.data as {
      row: Record<string, unknown>;
    };
    expect(patchedData.row).toEqual({
      id: 'row_1',
      schemaId: 'schema_1',
      schemaName: 'Case',
      tenantId: 'tenant_1',
      createdAt: '2026-01-01T00:00:00Z',
      updatedAt: '2026-01-02T00:00:00Z',
      status: 'closed',
      owner: 'Ada',
    });
    expect(patchedData.row).not.toBe(row);
  });

  it('returns the original object when no rendered row matches', () => {
    const block: ReportBlockResult = {
      type: 'table',
      status: 'ready',
      data: {
        columns: ['id', 'schemaId', 'status'],
        rows: [{ id: 'row_2', schemaId: 'schema_1', status: 'open' }],
      },
    };

    const patched = patchReportWritebackQueryData(block, {
      schemaId: 'schema_1',
      instanceId: 'row_1',
      field: 'status',
      value: 'closed',
    });

    expect(patched).toBe(block);
  });
});
