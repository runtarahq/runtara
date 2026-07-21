import { QueryClient } from '@tanstack/react-query';
import { describe, expect, it } from 'vitest';
import { queryKeys } from '@/shared/queries/query-keys';
import type { ReportRenderRequest, ReportRenderResponse } from './types';
import { cacheReportActionRender } from './reportActionRender';

describe('cacheReportActionRender', () => {
  it('seeds both the submitted and canonical render keys before URL navigation', () => {
    const client = new QueryClient();
    const request: ReportRenderRequest = {
      filters: {},
      viewId: 'intake',
      timezone: 'Europe/Warsaw',
    };
    const render: ReportRenderResponse = {
      success: true,
      report: { id: 'report-1', definitionVersion: 1 },
      resolvedFilters: {},
      blocks: {},
      navigation: {
        requestedViewId: 'intake',
        activeViewId: 'review',
        group: {
          id: 'case-stage',
          mode: 'stages',
          currentViewId: 'review',
          accessibleViewIds: ['review'],
        },
      },
      errors: [],
    };

    const result = cacheReportActionRender(client, 'report-1', request, {
      id: 'instance-1',
      workflowId: 'advance-stage',
      instanceId: 'instance-1',
      status: 'completed',
      render,
      canonicalViewId: 'review',
    });

    expect(result?.canonicalViewId).toBe('review');
    expect(
      client.getQueryData(queryKeys.reports.render('report-1', request))
    ).toBe(render);
    expect(
      client.getQueryData(
        queryKeys.reports.render('report-1', {
          ...request,
          viewId: 'review',
        })
      )
    ).toEqual(
      expect.objectContaining({
        navigation: expect.objectContaining({ requestedViewId: 'review' }),
      })
    );
  });
});
