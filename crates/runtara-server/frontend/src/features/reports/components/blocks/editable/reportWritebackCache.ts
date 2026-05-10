import type { QueryKey } from '@tanstack/react-query';
import type { Instance } from '@/generated/RuntaraRuntimeApi';
import type { ReportBlockResult, ReportRenderResponse } from '../../../types';

export type ReportWritebackPatch = {
  schemaId: string;
  instanceId: string;
  field: string;
  value: unknown;
  instance?: Instance;
};

export type ReportWritebackSnapshot = [QueryKey, unknown][];

type TableColumnLike = string | { key?: unknown; field?: unknown };

type TableDataLike = {
  columns?: TableColumnLike[];
  rows?: unknown[];
};

type CardDataLike = {
  row?: unknown;
};

export function patchReportWritebackQueryData<T>(
  data: T,
  patch: ReportWritebackPatch
): T {
  if (isReportRenderResponse(data)) {
    return patchRenderResponse(data, patch) as T;
  }
  if (isReportBlockResult(data)) {
    return patchBlockResult(data, patch) as T;
  }
  return data;
}

function patchRenderResponse(
  response: ReportRenderResponse,
  patch: ReportWritebackPatch
): ReportRenderResponse {
  let changed = false;
  const nextBlocks: ReportRenderResponse['blocks'] = {};

  for (const [blockId, blockResult] of Object.entries(response.blocks)) {
    const nextBlockResult = patchBlockResult(blockResult, patch);
    nextBlocks[blockId] = nextBlockResult;
    if (nextBlockResult !== blockResult) {
      changed = true;
    }
  }

  if (!changed) return response;
  return {
    ...response,
    blocks: nextBlocks,
  };
}

function patchBlockResult(
  result: ReportBlockResult,
  patch: ReportWritebackPatch
): ReportBlockResult {
  const nextData = patchBlockData(result.data, patch);
  if (nextData === result.data) return result;
  return {
    ...result,
    data: nextData,
  };
}

function patchBlockData(data: unknown, patch: ReportWritebackPatch): unknown {
  if (!isRecord(data)) return data;

  const tableData = data as TableDataLike;
  if (Array.isArray(tableData.rows)) {
    const nextRows = patchRows(tableData.rows, tableData.columns, patch);
    if (nextRows !== tableData.rows) {
      return {
        ...data,
        rows: nextRows,
      };
    }
  }

  const cardData = data as CardDataLike;
  if (isRecord(cardData.row)) {
    const nextRow = patchObjectRow(cardData.row, patch);
    if (nextRow !== cardData.row) {
      return {
        ...data,
        row: nextRow,
      };
    }
  }

  return data;
}

function patchRows(
  rows: unknown[],
  columns: TableColumnLike[] | undefined,
  patch: ReportWritebackPatch
): unknown[] {
  let changed = false;
  const columnKeys = columns?.map(getColumnKey) ?? [];
  const nextRows = rows.map((row) => {
    if (isRecord(row)) {
      const nextRow = patchObjectRow(row, patch);
      if (nextRow !== row) changed = true;
      return nextRow;
    }
    if (Array.isArray(row)) {
      const nextRow = patchArrayRow(row, columnKeys, patch);
      if (nextRow !== row) changed = true;
      return nextRow;
    }
    return row;
  });

  return changed ? nextRows : rows;
}

function patchObjectRow(
  row: Record<string, unknown>,
  patch: ReportWritebackPatch
): Record<string, unknown> {
  if (!isTargetRow(row, patch)) return row;

  const rowPatch = getRowPatch(patch);
  let changed = false;
  const nextRow = { ...row };

  for (const [key, value] of Object.entries(rowPatch)) {
    if (!Object.is(nextRow[key], value)) {
      nextRow[key] = value;
      changed = true;
    }
  }

  return changed ? nextRow : row;
}

function patchArrayRow(
  row: unknown[],
  columnKeys: string[],
  patch: ReportWritebackPatch
): unknown[] {
  const idIndex = columnKeys.indexOf('id');
  const schemaIdIndex = columnKeys.indexOf('schemaId');
  if (idIndex < 0 || schemaIdIndex < 0) return row;
  if (
    row[idIndex] !== patch.instanceId ||
    row[schemaIdIndex] !== patch.schemaId
  ) {
    return row;
  }

  const rowPatch = getRowPatch(patch);
  let changed = false;
  const nextRow = [...row];
  for (const [key, value] of Object.entries(rowPatch)) {
    const index = columnKeys.indexOf(key);
    if (index < 0 || Object.is(nextRow[index], value)) continue;
    nextRow[index] = value;
    changed = true;
  }

  return changed ? nextRow : row;
}

function getRowPatch(patch: ReportWritebackPatch): Record<string, unknown> {
  const flattenedInstance = flattenInstanceForReportRow(patch.instance);
  return {
    ...flattenedInstance,
    id: patch.instanceId,
    schemaId: patch.schemaId,
    [patch.field]: Object.prototype.hasOwnProperty.call(
      flattenedInstance,
      patch.field
    )
      ? flattenedInstance[patch.field]
      : patch.value,
  };
}

function flattenInstanceForReportRow(
  instance: Instance | undefined
): Record<string, unknown> {
  if (!instance) return {};

  const row: Record<string, unknown> = {
    id: instance.id,
    tenantId: instance.tenantId,
    createdAt: instance.createdAt,
    updatedAt: instance.updatedAt,
  };
  if (typeof instance.schemaId === 'string') {
    row.schemaId = instance.schemaId;
  }
  if (typeof instance.schemaName === 'string') {
    row.schemaName = instance.schemaName;
  }
  if (isRecord(instance.properties)) {
    Object.assign(row, instance.properties);
  }
  if (isRecord(instance.computed)) {
    Object.assign(row, instance.computed);
  }
  return row;
}

function isTargetRow(
  row: Record<string, unknown>,
  patch: ReportWritebackPatch
): boolean {
  return row.id === patch.instanceId && row.schemaId === patch.schemaId;
}

function getColumnKey(column: TableColumnLike): string {
  if (typeof column === 'string') return column;
  if (typeof column.key === 'string') return column.key;
  if (typeof column.field === 'string') return column.field;
  return '';
}

function isReportRenderResponse(value: unknown): value is ReportRenderResponse {
  return (
    isRecord(value) &&
    isRecord(value.report) &&
    isRecord(value.blocks) &&
    typeof value.success === 'boolean'
  );
}

function isReportBlockResult(value: unknown): value is ReportBlockResult {
  return (
    isRecord(value) &&
    typeof value.type === 'string' &&
    typeof value.status === 'string' &&
    'data' in value
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value));
}
