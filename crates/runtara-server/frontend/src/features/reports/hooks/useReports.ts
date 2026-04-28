import { useQueryClient } from '@tanstack/react-query';
import { useCustomMutation, useCustomQuery } from '@/shared/hooks/api';
import { queryKeys } from '@/shared/queries/query-keys';
import {
  createReport,
  deleteReport,
  getReport,
  getReportBlockData,
  listReports,
  renderReport,
  updateReport,
} from '../queries';
import {
  CreateReportRequest,
  ReportBlockDataRequest,
  ReportBlockResult,
  ReportDto,
  ReportRenderRequest,
  ReportRenderResponse,
  ReportSummary,
  UpdateReportRequest,
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

export function useDeleteReport() {
  const queryClient = useQueryClient();

  return useCustomMutation<void, string>({
    mutationFn: (token, reportId) => deleteReport(token, reportId),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: queryKeys.reports.lists() });
    },
  });
}
