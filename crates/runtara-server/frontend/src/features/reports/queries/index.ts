import { RuntimeREST } from '@/shared/queries';
import { createAuthHeaders } from '@/shared/queries/utils';
import {
  CreateReportRequest,
  ReportBlockDataRequest,
  ReportBlockResult,
  ReportDatasetQueryRequest,
  ReportDatasetQueryResponse,
  ReportDto,
  ReportFilterOptionsRequest,
  ReportFilterOptionsResponse,
  ReportRenderRequest,
  ReportRenderResponse,
  ReportSummary,
  UpdateReportRequest,
} from '../types';

export async function listReports(token: string): Promise<ReportSummary[]> {
  const result = await RuntimeREST.instance.get(
    '/api/runtime/reports',
    createAuthHeaders(token)
  );
  return result.data.reports ?? [];
}

export async function getReport(
  token: string,
  context: { queryKey: readonly unknown[] }
): Promise<ReportDto | null> {
  const reportId = context.queryKey[2];
  if (typeof reportId !== 'string' || reportId.length === 0) {
    return null;
  }

  const result = await RuntimeREST.instance.get(
    `/api/runtime/reports/${encodeURIComponent(reportId)}`,
    createAuthHeaders(token)
  );
  return result.data.report ?? null;
}

export async function createReport(
  token: string,
  request: CreateReportRequest
): Promise<ReportDto> {
  const result = await RuntimeREST.instance.post(
    '/api/runtime/reports',
    request,
    createAuthHeaders(token)
  );
  return result.data.report;
}

export async function updateReport(
  token: string,
  request: { id: string; data: UpdateReportRequest }
): Promise<ReportDto> {
  const result = await RuntimeREST.instance.put(
    `/api/runtime/reports/${encodeURIComponent(request.id)}`,
    request.data,
    createAuthHeaders(token)
  );
  return result.data.report;
}

export async function deleteReport(
  token: string,
  reportId: string
): Promise<void> {
  await RuntimeREST.instance.delete(
    `/api/runtime/reports/${encodeURIComponent(reportId)}`,
    createAuthHeaders(token)
  );
}

export async function renderReport(
  token: string,
  context: { queryKey: readonly unknown[] }
): Promise<ReportRenderResponse | null> {
  const reportId = context.queryKey[2];
  const request = context.queryKey[4];

  if (typeof reportId !== 'string' || reportId.length === 0) {
    return null;
  }

  const result = await RuntimeREST.instance.post(
    `/api/runtime/reports/${encodeURIComponent(reportId)}/render`,
    request as ReportRenderRequest,
    createAuthHeaders(token)
  );
  return result.data;
}

export async function getReportBlockData(
  token: string,
  context: { queryKey: readonly unknown[] }
): Promise<ReportBlockResult | null> {
  const reportId = context.queryKey[2];
  const blockId = context.queryKey[4];
  const request = context.queryKey[5] as
    | (Omit<ReportBlockDataRequest, 'id'> & {
        filters: Record<string, unknown>;
        timezone?: string;
      })
    | undefined;

  if (
    typeof reportId !== 'string' ||
    reportId.length === 0 ||
    typeof blockId !== 'string' ||
    blockId.length === 0
  ) {
    return null;
  }

  const result = await RuntimeREST.instance.post(
    `/api/runtime/reports/${encodeURIComponent(reportId)}/blocks/${encodeURIComponent(blockId)}/data`,
    request ?? { filters: {} },
    createAuthHeaders(token)
  );
  return result.data;
}

export async function getReportFilterOptions(
  token: string,
  context: { queryKey: readonly unknown[] }
): Promise<ReportFilterOptionsResponse | null> {
  const reportId = context.queryKey[2];
  const filterId = context.queryKey[4];
  const request = context.queryKey[5] as ReportFilterOptionsRequest | undefined;

  if (
    typeof reportId !== 'string' ||
    reportId.length === 0 ||
    typeof filterId !== 'string' ||
    filterId.length === 0
  ) {
    return null;
  }

  const result = await RuntimeREST.instance.post(
    `/api/runtime/reports/${encodeURIComponent(reportId)}/filters/${encodeURIComponent(filterId)}/options`,
    request ?? { filters: {} },
    createAuthHeaders(token)
  );
  return result.data;
}

export async function queryReportDataset(
  token: string,
  context: { queryKey: readonly unknown[] }
): Promise<ReportDatasetQueryResponse | null> {
  const reportId = context.queryKey[2];
  const datasetId = context.queryKey[4];
  const request = context.queryKey[5] as ReportDatasetQueryRequest | undefined;

  if (
    typeof reportId !== 'string' ||
    reportId.length === 0 ||
    typeof datasetId !== 'string' ||
    datasetId.length === 0
  ) {
    return null;
  }

  const result = await RuntimeREST.instance.post(
    `/api/runtime/reports/${encodeURIComponent(reportId)}/datasets/${encodeURIComponent(datasetId)}/query`,
    request ?? { dimensions: [], measures: [] },
    createAuthHeaders(token)
  );
  return result.data;
}
