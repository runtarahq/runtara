import type { QueryClient } from '@tanstack/react-query';
import { queryKeys } from '@/shared/queries/query-keys';
import type { ReportRenderRequest, ReportRenderResponse } from './types';
import type { ReportWorkflowActionResult } from './components/blocks/useReportWorkflowAction';

export function cacheReportActionRender(
  queryClient: QueryClient,
  reportId: string,
  renderRequest: ReportRenderRequest,
  actionResult: ReportWorkflowActionResult
): { render: ReportRenderResponse; canonicalViewId?: string } | null {
  const rendered = actionResult.render;
  if (!rendered) return null;

  queryClient.setQueryData(
    queryKeys.reports.render(reportId, renderRequest),
    rendered
  );
  const canonicalViewId =
    actionResult.canonicalViewId ??
    rendered.navigation?.activeViewId ??
    undefined;
  if (!canonicalViewId) return { render: rendered };

  const canonicalRequest = { ...renderRequest, viewId: canonicalViewId };
  const canonicalRender: ReportRenderResponse = {
    ...rendered,
    navigation: rendered.navigation
      ? { ...rendered.navigation, requestedViewId: canonicalViewId }
      : rendered.navigation,
  };
  queryClient.setQueryData(
    queryKeys.reports.render(reportId, canonicalRequest),
    canonicalRender
  );
  return { render: canonicalRender, canonicalViewId };
}
