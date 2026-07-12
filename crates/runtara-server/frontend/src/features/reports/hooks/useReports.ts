import { useQueryClient } from '@tanstack/react-query';
import { useCustomMutation, useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import {
  createReport,
  deleteReport,
  editReport,
  getReport,
  getReportBlockData,
  getReportLookupOptions,
  listReports,
  previewReport,
  queryReportDataset,
  renderReport,
  validateReport,
  updateReport,
} from '../queries';
import {
  CreateReportRequest,
  ReportBlockDataRequest,
  ReportBlockResult,
  ReportDatasetQueryRequest,
  ReportDatasetQueryResponse,
  ReportDto,
  ReportEditOp,
  ReportLookupOptionsRequest,
  ReportLookupOptionsResponse,
  ReportPreviewRequest,
  ReportRenderRequest,
  ReportRenderResponse,
  ReportSummary,
  UpdateReportRequest,
  ValidateReportRequest,
  ValidateReportResponse,
} from '../types';

export function useReports() {
  return useCustomQuery<ReportSummary[]>({
    queryKey: queryKeys.reports.lists(),
    queryFn: (token) => listReports(token),
  });
}

export function useReport(reportId: string | undefined) {
  return useCustomQuery<ReportDto | null>({
    queryKey: queryKeys.reports.byId(reportId ?? ''),
    queryFn: (token, context) => getReport(token, context),
    enabled: Boolean(reportId),
  });
}

export function useReportRender(
  reportId: string | undefined,
  request: ReportRenderRequest | undefined,
  enabled: boolean
) {
  return useCustomQuery<ReportRenderResponse | null>({
    queryKey: queryKeys.reports.render(reportId ?? '', request ?? {}),
    queryFn: (token, context) => renderReport(token, context),
    enabled: Boolean(reportId && request && enabled),
  });
}

export function useReportPreview(
  request: ReportPreviewRequest | undefined,
  enabled: boolean
) {
  return useCustomQuery<ReportRenderResponse | null>({
    queryKey: queryKeys.reports.preview(request ?? {}),
    queryFn: (token, context) => previewReport(token, context),
    enabled: Boolean(request && enabled),
  });
}

export function useReportBlockData(
  reportId: string | undefined,
  blockId: string | undefined,
  request:
    | (Omit<ReportBlockDataRequest, 'id'> & {
        filters: Record<string, unknown>;
        timezone?: string;
      })
    | undefined,
  enabled: boolean
) {
  return useCustomQuery<ReportBlockResult | null>({
    queryKey: queryKeys.reports.block(
      reportId ?? '',
      blockId ?? '',
      request ?? {}
    ),
    queryFn: (token, context) => getReportBlockData(token, context),
    enabled: Boolean(reportId && blockId && request && enabled),
  });
}

export function useReportLookupOptions(
  reportId: string | undefined,
  blockId: string | undefined,
  field: string | undefined,
  request: ReportLookupOptionsRequest | undefined,
  enabled: boolean
) {
  return useCustomQuery<ReportLookupOptionsResponse | null>({
    queryKey: queryKeys.reports.lookupOptions(
      reportId ?? '',
      blockId ?? '',
      field ?? '',
      request ?? {}
    ),
    queryFn: (token, context) => getReportLookupOptions(token, context),
    enabled: Boolean(reportId && blockId && field && request && enabled),
  });
}

export function useReportDatasetQuery(
  reportId: string | undefined,
  datasetId: string | undefined,
  request: ReportDatasetQueryRequest | undefined,
  enabled: boolean
) {
  return useCustomQuery<ReportDatasetQueryResponse | null>({
    queryKey: queryKeys.reports.dataset(
      reportId ?? '',
      datasetId ?? '',
      request ?? {}
    ),
    queryFn: (token, context) => queryReportDataset(token, context),
    enabled: Boolean(reportId && datasetId && request && enabled),
  });
}

export function useCreateReport() {
  const queryClient = useQueryClient();

  return useCustomMutation<ReportDto, CreateReportRequest>({
    mutationFn: (token, request) => createReport(token, request),
    onSuccess: (report) => {
      queryClient.invalidateQueries({ queryKey: queryKeys.reports.lists() });
      queryClient.setQueryData(queryKeys.reports.byId(report.id), report);
    },
  });
}

export function useValidateReport() {
  return useCustomMutation<ValidateReportResponse, ValidateReportRequest>({
    mutationFn: (token, request) => validateReport(token, request),
  });
}

export function useUpdateReport() {
  const queryClient = useQueryClient();

  return useCustomMutation<
    ReportDto,
    { id: string; data: UpdateReportRequest }
  >({
    mutationFn: (token, request) => updateReport(token, request),
    onSuccess: (report) => {
      queryClient.invalidateQueries({ queryKey: queryKeys.reports.lists() });
      queryClient.setQueryData(queryKeys.reports.byId(report.id), report);
      queryClient.invalidateQueries({
        queryKey: queryKeys.reports.byId(report.id),
      });
    },
  });
}

/** Canonical partial-mutation hook backed by `POST /reports/{id}/edit`.
 *  Applies a batch of `ReportEditOp`s atomically server-side. Prefer this
 *  over `useUpdateReport` (full PUT) for targeted edits — MCP authoring
 *  flows and any future per-block UIs should route through here. */
export function useEditReport() {
  const queryClient = useQueryClient();

  return useCustomMutation<ReportDto, { id: string; ops: ReportEditOp[] }>({
    mutationFn: (token, request) => editReport(token, request),
    onSuccess: (report) => {
      queryClient.invalidateQueries({ queryKey: queryKeys.reports.lists() });
      queryClient.setQueryData(queryKeys.reports.byId(report.id), report);
      queryClient.invalidateQueries({
        queryKey: queryKeys.reports.byId(report.id),
      });
    },
  });
}

export function useDeleteReport() {
  const queryClient = useQueryClient();

  return useCustomMutation<void, string>({
    mutationFn: (token, reportId) => deleteReport(token, reportId),
    onSuccess: (_data, reportId) => {
      queryClient.invalidateQueries({ queryKey: queryKeys.reports.lists() });
      queryClient.removeQueries({ queryKey: queryKeys.reports.byId(reportId) });
    },
  });
}
