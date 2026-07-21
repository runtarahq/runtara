import type { Page, Route } from '@playwright/test';
import {
  buildObjectModelConnection,
  expect,
  test,
  type MockApi,
} from '../../../fixtures';
import { appPath } from '../../../utils/app-path';
import type {
  ReportBlockResult,
  ReportDefinition,
  ReportDto,
  ReportRenderResponse,
  UpdateReportRequest,
} from '../../../../src/features/reports/types';

const TAB_REPORT_ID = 'report_tabs_navigation';
const STAGE_REPORT_ID = 'report_stage_navigation';
const AUTHOR_REPORT_ID = 'report_navigation_authoring';
const WORKFLOW_STAGE_REPORT_ID = 'report_workflow_stage_navigation';
const WORKFLOW_TABLE_REPORT_ID = 'report_workflow_table_interactions';

function runtimeUrl(suffix: string): RegExp {
  const escaped = suffix.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return new RegExp(`/api/runtime(?:/[^/]+)?/${escaped}(?:\\?[^/]*)?$`);
}

async function fulfill(route: Route, body: unknown, status = 200) {
  await route.fulfill({
    status,
    contentType: 'application/json',
    body: JSON.stringify(body),
  });
}

async function gotoAppRoute(page: Page, path: string) {
  await page.goto(appPath('/'));
  await page.evaluate((routePath) => {
    const basePath = new URL(document.baseURI).pathname.replace(/\/$/, '');
    const normalizedPath = routePath.startsWith('/')
      ? routePath
      : `/${routePath}`;
    window.history.pushState({}, '', `${basePath}${normalizedPath}`);
    window.dispatchEvent(new PopStateEvent('popstate'));
  }, path);
}

async function expectView(page: Page, viewId: string) {
  await expect
    .poll(() => new URL(page.url()).searchParams.get('view'))
    .toBe(viewId);
}

function markdownBlock(id: string, title: string) {
  return {
    id,
    type: 'markdown' as const,
    title,
    source: { schema: '', mode: 'filter' as const },
    markdown: { content: `# ${title}` },
  };
}

function view(id: string, title: string, blockId: string) {
  return {
    id,
    title,
    layout: {
      id: `${id}_root`,
      columns: 1,
      items: [
        {
          id: `${id}_item`,
          child: { id: `${id}_node`, type: 'block' as const, blockId },
        },
      ],
    },
  };
}

function reportFor(id: string, definition: ReportDefinition): ReportDto {
  return {
    id,
    slug: id,
    name: id === AUTHOR_REPORT_ID ? 'Navigation authoring' : 'View navigation',
    description: null,
    tags: [],
    status: 'published',
    definitionVersion: 1,
    definition,
    createdAt: '2026-07-21T00:00:00Z',
    updatedAt: '2026-07-21T00:00:00Z',
  };
}

function markdownResult(title: string): ReportBlockResult {
  return {
    type: 'markdown',
    status: 'ready',
    data: { content: `# ${title}` },
  };
}

async function bootstrapReport(
  page: Page,
  mockApi: MockApi,
  reportId: string,
  definition: ReportDefinition,
  render: (request: { viewId?: string }) => ReportRenderResponse
) {
  await mockApi.bootstrap(page);
  await mockApi.connections.list(page, [
    buildObjectModelConnection({ id: 'conn_object_model_default' }),
  ]);
  await mockApi.objects.schemas.list(page, []);
  await mockApi.raw(page, runtimeUrl(`reports/${reportId}`), {
    report: reportFor(reportId, definition),
  });
  await mockApi.raw(
    page,
    runtimeUrl(`reports/${reportId}/render`),
    async (route) => {
      await fulfill(
        route,
        render(route.request().postDataJSON() as { viewId?: string })
      );
    }
  );
}

test.describe('report view navigation (mocked)', () => {
  test('workflow action applies the endpoint render without polling or a second report render', async ({
    page,
    mockApi,
  }) => {
    const caseCard = {
      id: 'case_card',
      type: 'card' as const,
      title: 'Case',
      source: { schema: 'ReportStageCase', mode: 'filter' as const },
      card: {
        groups: [
          {
            id: 'actions',
            fields: [
              {
                field: 'advance_action',
                label: 'Advance',
                kind: 'workflow_button' as const,
                workflowAction: {
                  id: 'advance_stage',
                  workflowId: 'advance-case',
                  label: 'Advance',
                  successMessage: 'Stage advanced',
                  context: { mode: 'row' as const },
                },
              },
            ],
          },
        ],
      },
    };
    const definition: ReportDefinition = {
      definitionVersion: 1,
      layout: { id: 'root', items: [] },
      filters: [],
      blocks: [caseCard],
      views: [
        view('intake', 'Intake', caseCard.id),
        view('review', 'Review', caseCard.id),
      ],
      viewGroups: [
        {
          id: 'case_stage',
          mode: 'stages',
          stages: [
            { viewId: 'intake', value: 'intake' },
            { viewId: 'review', value: 'review' },
          ],
          currentFrom: {
            type: 'block',
            blockId: caseCard.id,
            field: 'stage',
          },
          access: 'current_only',
          followCurrentOnAdvance: true,
        },
      ],
    };
    const renderFor = (stage: 'intake' | 'review'): ReportRenderResponse => ({
      success: true,
      report: { id: WORKFLOW_STAGE_REPORT_ID, definitionVersion: 1 },
      resolvedFilters: {},
      blocks: {
        [caseCard.id]: {
          type: 'card',
          status: 'ready',
          data: {
            row: {
              id: 'case-1',
              stage,
              advance_action: stage,
            },
          },
        },
      },
      navigation: {
        requestedViewId: stage,
        activeViewId: stage,
        group: {
          id: 'case_stage',
          mode: 'stages',
          currentViewId: stage,
          accessibleViewIds: [stage],
        },
      },
      errors: [],
    });
    let reportRenderRequests = 0;
    const workflowRequests: string[] = [];
    page.on('request', (request) => {
      if (/\/api\/runtime(?:\/[^/]+)?\/workflows\//.test(request.url())) {
        workflowRequests.push(request.url());
      }
    });

    await bootstrapReport(
      page,
      mockApi as MockApi,
      WORKFLOW_STAGE_REPORT_ID,
      definition,
      () => {
        reportRenderRequests += 1;
        return renderFor('intake');
      }
    );
    await mockApi.raw(
      page,
      runtimeUrl(
        `reports/${WORKFLOW_STAGE_REPORT_ID}/blocks/${caseCard.id}/workflow-actions/advance_stage/execute`
      ),
      async (route) => {
        expect(route.request().headers()['idempotency-key']).toBeTruthy();
        await fulfill(route, {
          completedWithinWait: true,
          execution: {
            workflowId: 'advance-case',
            version: 2,
            instanceId: 'instance-1',
            status: 'completed',
            durationMs: 45,
          },
          canonicalViewId: 'review',
          render: renderFor('review'),
        });
      }
    );

    await gotoAppRoute(
      page,
      `/reports/${WORKFLOW_STAGE_REPORT_ID}?view=intake`
    );
    await expect(page.getByRole('button', { name: 'Advance' })).toBeVisible();
    expect(reportRenderRequests).toBe(1);

    await page.getByRole('button', { name: 'Advance' }).click();

    await expectView(page, 'review');
    await expect(page.getByText('Stage advanced')).toBeVisible();
    await expect(page.getByRole('button', { name: /Review/ })).toHaveAttribute(
      'aria-current',
      'step'
    );
    expect(reportRenderRequests).toBe(1);
    expect(workflowRequests).toEqual([]);
  });

  test('table inline editing and a row workflow action refresh the record in place', async ({
    page,
    mockApi,
  }) => {
    const casesTable = {
      id: 'cases_table',
      type: 'table' as const,
      title: 'Workflow cases',
      source: { schema: 'ReportStageCase', mode: 'filter' as const },
      table: {
        columns: [
          { field: 'case_ref', label: 'Reference' },
          {
            field: 'owner',
            label: 'Owner',
            editable: true,
            editor: { kind: 'text' as const },
          },
          { field: 'stage', label: 'Stage', format: 'pill' },
          {
            field: 'advance_action',
            label: 'Advance',
            type: 'workflow_button' as const,
            workflowAction: {
              id: 'advance_case_row',
              workflowId: 'advance-case',
              label: 'Advance',
              successMessage: 'Case advanced',
              context: { mode: 'row' as const },
            },
          },
        ],
        defaultSort: [],
      },
    };
    const definition: ReportDefinition = {
      definitionVersion: 1,
      layout: {
        id: 'root',
        columns: 1,
        items: [
          {
            id: 'cases_item',
            child: {
              id: 'cases_node',
              type: 'block',
              blockId: casesTable.id,
            },
          },
        ],
      },
      filters: [],
      blocks: [casesTable],
    };
    let owner = 'Avery Stone';
    let stage = 'intake';
    let reportRenderRequests = 0;
    const workflowRequests: string[] = [];
    const renderTable = (): ReportRenderResponse => ({
      success: true,
      report: { id: WORKFLOW_TABLE_REPORT_ID, definitionVersion: 1 },
      resolvedFilters: {},
      blocks: {
        [casesTable.id]: {
          type: 'table',
          status: 'ready',
          data: {
            columns: [
              { key: 'case_ref', label: 'Reference' },
              { key: 'owner', label: 'Owner' },
              { key: 'stage', label: 'Stage', format: 'pill' },
              { key: 'advance_action', label: 'Advance' },
            ],
            rows: [
              {
                id: 'case-1',
                schemaId: 'schema-1',
                case_ref: 'FLOW-TBL-001',
                owner,
                stage,
              },
            ],
            totalCount: 1,
            offset: 0,
            limit: 50,
            hasNextPage: false,
          },
        },
      },
      errors: [],
    });
    page.on('request', (request) => {
      if (/\/api\/runtime(?:\/[^/]+)?\/workflows\//.test(request.url())) {
        workflowRequests.push(request.url());
      }
    });

    await bootstrapReport(
      page,
      mockApi as MockApi,
      WORKFLOW_TABLE_REPORT_ID,
      definition,
      () => {
        reportRenderRequests += 1;
        return renderTable();
      }
    );
    await mockApi.raw(
      page,
      runtimeUrl('object-model/instances/schema-1/case-1'),
      async (route) => {
        if (route.request().method() === 'PUT') {
          owner = (
            route.request().postDataJSON() as { properties: { owner: string } }
          ).properties.owner;
          await fulfill(route, {
            success: true,
            message: 'Instance updated successfully',
          });
          return;
        }
        await fulfill(route, {
          success: true,
          instance: {
            id: 'case-1',
            tenantId: 'tenant-1',
            schemaId: 'schema-1',
            schemaName: 'ReportStageCase',
            properties: { case_ref: 'FLOW-TBL-001', owner, stage },
            createdAt: '2026-07-21T00:00:00Z',
            updatedAt: '2026-07-21T00:01:00Z',
          },
        });
      }
    );
    await mockApi.raw(
      page,
      runtimeUrl(
        `reports/${WORKFLOW_TABLE_REPORT_ID}/blocks/${casesTable.id}/workflow-actions/advance_case_row/execute`
      ),
      async (route) => {
        const body = route.request().postDataJSON() as {
          trigger: { row: { owner: string; stage: string } };
        };
        expect(body.trigger.row).toMatchObject({ owner, stage: 'intake' });
        stage = 'review';
        await fulfill(route, {
          completedWithinWait: true,
          execution: {
            workflowId: 'advance-case',
            version: 2,
            instanceId: 'instance-table-1',
            status: 'completed',
            durationMs: 38,
          },
          render: renderTable(),
        });
      }
    );

    await gotoAppRoute(page, `/reports/${WORKFLOW_TABLE_REPORT_ID}`);
    const row = page.getByRole('row').filter({ hasText: 'FLOW-TBL-001' });
    await expect(row).toContainText('Avery Stone');
    expect(reportRenderRequests).toBe(1);

    await row.getByRole('button', { name: 'Edit cell' }).click();
    const ownerInput = row.getByRole('textbox');
    await ownerInput.fill('Morgan Reed');
    await ownerInput.press('Enter');

    await expect(page.getByText('Updated')).toBeVisible();
    await expect(row).toContainText('Morgan Reed');
    await expect.poll(() => reportRenderRequests).toBe(2);

    await row.getByRole('button', { name: 'Advance' }).click();

    await expect(page.getByText('Case advanced')).toBeVisible();
    await expect(row).toContainText('Review');
    expect(reportRenderRequests).toBe(2);
    expect(workflowRequests).toEqual([]);
  });

  test('tabs change the active detail, scope filters, and preserve browser history', async ({
    page,
    mockApi,
  }) => {
    const overview = markdownBlock('overview_block', 'Overview content');
    const activity = markdownBlock('activity_block', 'Activity content');
    const definition: ReportDefinition = {
      definitionVersion: 1,
      layout: { id: 'root', items: [] },
      filters: [
        {
          id: 'overview_search',
          label: 'Overview search',
          type: 'search',
          appliesTo: [{ blockId: overview.id }],
        },
      ],
      blocks: [overview, activity],
      views: [
        view('overview', 'Overview', overview.id),
        view('activity', 'Activity', activity.id),
      ],
      viewGroups: [
        {
          id: 'details',
          mode: 'tabs',
          viewIds: ['overview', 'activity'],
          access: 'all',
        },
      ],
    };

    await bootstrapReport(
      page,
      mockApi as MockApi,
      TAB_REPORT_ID,
      definition,
      ({ viewId = 'overview' }) => {
        const activeViewId = viewId === 'activity' ? 'activity' : 'overview';
        const activeBlock =
          activeViewId === 'activity' ? activity.id : overview.id;
        const activeTitle =
          activeViewId === 'activity' ? 'Activity content' : 'Overview content';
        return {
          success: true,
          report: { id: TAB_REPORT_ID, definitionVersion: 1 },
          resolvedFilters: {},
          blocks: { [activeBlock]: markdownResult(activeTitle) },
          navigation: {
            requestedViewId: viewId,
            activeViewId,
            group: {
              id: 'details',
              mode: 'tabs',
              accessibleViewIds: ['overview', 'activity'],
            },
          },
          errors: [],
        };
      }
    );

    await gotoAppRoute(page, `/reports/${TAB_REPORT_ID}?view=overview`);
    await expect(page.getByRole('tab', { name: 'Overview' })).toHaveAttribute(
      'data-state',
      'active'
    );
    await expect(page.getByPlaceholder('Overview search')).toBeVisible();
    await expect(
      page.getByRole('heading', { name: 'Overview content', level: 1 })
    ).toBeVisible();

    await page.getByRole('tab', { name: 'Activity' }).click();
    await expectView(page, 'activity');
    await expect(page.getByRole('tab', { name: 'Activity' })).toHaveAttribute(
      'data-state',
      'active'
    );
    await expect(page.getByPlaceholder('Overview search')).toHaveCount(0);
    await expect(
      page.getByRole('heading', { name: 'Activity content', level: 1 })
    ).toBeVisible();

    await page.goBack();
    if (new URL(page.url()).searchParams.get('view') === 'activity') {
      await page.goBack();
    }
    await expectView(page, 'overview');
    await expect(
      page.getByRole('heading', { name: 'Overview content', level: 1 })
    ).toBeVisible();
  });

  test('stages correct future links, lock unavailable steps, and expose prior/next navigation', async ({
    page,
    mockApi,
  }) => {
    const blocks = {
      stage_a: markdownBlock('stage_a_block', 'Stage A content'),
      stage_b: markdownBlock('stage_b_block', 'Stage B content'),
      stage_c: markdownBlock('stage_c_block', 'Stage C content'),
    };
    const definition: ReportDefinition = {
      definitionVersion: 1,
      layout: { id: 'root', items: [] },
      filters: [{ id: 'stage', label: 'Stage', type: 'text', default: 'B' }],
      blocks: Object.values(blocks),
      views: [
        view('stage_a', 'Stage A', blocks.stage_a.id),
        view('stage_b', 'Stage B', blocks.stage_b.id),
        view('stage_c', 'Stage C', blocks.stage_c.id),
      ],
      viewGroups: [
        {
          id: 'approval',
          mode: 'stages',
          stages: [
            { viewId: 'stage_a', value: 'A' },
            { viewId: 'stage_b', value: 'B' },
            { viewId: 'stage_c', value: 'C' },
          ],
          currentFrom: { type: 'filter', filterId: 'stage' },
          access: 'through_current',
          showPreviousNext: true,
          followCurrentOnAdvance: true,
        },
      ],
    };
    await bootstrapReport(
      page,
      mockApi as MockApi,
      STAGE_REPORT_ID,
      definition,
      ({ viewId = 'approval' }) => {
        const currentViewId = 'stage_b';
        const accessibleViewIds = ['stage_a', 'stage_b'];
        const activeViewId = accessibleViewIds.includes(viewId)
          ? viewId
          : currentViewId;
        const activeBlock = blocks[activeViewId as keyof typeof blocks];
        return {
          success: true,
          report: { id: STAGE_REPORT_ID, definitionVersion: 1 },
          resolvedFilters: { stage: 'B' },
          blocks: {
            [activeBlock.id]: markdownResult(
              activeBlock.title ?? activeBlock.id
            ),
          },
          navigation: {
            requestedViewId: viewId,
            activeViewId,
            group: {
              id: 'approval',
              mode: 'stages',
              currentViewId,
              accessibleViewIds,
            },
          },
          errors: [],
        };
      }
    );

    await gotoAppRoute(page, `/reports/${STAGE_REPORT_ID}?view=stage_c`);
    await expectView(page, 'stage_b');
    await expect(page.getByRole('button', { name: /Stage B/ })).toHaveAttribute(
      'aria-current',
      'step'
    );
    await expect(page.getByRole('button', { name: /Stage C/ })).toBeDisabled();
    await expect(
      page.getByRole('button', { name: 'Next stage' })
    ).toBeDisabled();

    await page.getByRole('button', { name: /Stage A/ }).click();
    await expectView(page, 'stage_a');
    await expect(
      page.getByRole('button', { name: 'Next stage' })
    ).toBeEnabled();
    await page.getByRole('button', { name: 'Next stage' }).click();
    await expectView(page, 'stage_b');
    await expect(
      page.getByRole('heading', { name: 'Stage B content', level: 1 })
    ).toBeVisible();
  });

  test('builder edits a detail layout and saves an authored stage group', async ({
    page,
    mockApi,
  }) => {
    const definition: ReportDefinition = {
      definitionVersion: 1,
      layout: { id: 'root', columns: 1, rows: 1, items: [] },
      filters: [{ id: 'stage', label: 'Stage', type: 'text' }],
      blocks: [],
      views: [
        view('draft', 'Draft', 'unused_draft'),
        view('review', 'Review', 'unused_review'),
        view('complete', 'Complete', 'unused_complete'),
      ].map((candidate) => ({
        ...candidate,
        layout: { ...candidate.layout, items: [] },
      })),
    };
    let saved: UpdateReportRequest | null = null;

    await mockApi.bootstrap(page);
    await mockApi.connections.list(page, [
      buildObjectModelConnection({ id: 'conn_object_model_default' }),
    ]);
    await mockApi.objects.schemas.list(page, []);
    await mockApi.raw(
      page,
      runtimeUrl(`reports/${AUTHOR_REPORT_ID}`),
      async (route) => {
        if (route.request().method() === 'PUT') {
          saved = route.request().postDataJSON() as UpdateReportRequest;
          await fulfill(route, {
            report: reportFor(AUTHOR_REPORT_ID, saved.definition),
          });
          return;
        }
        await fulfill(route, {
          report: reportFor(AUTHOR_REPORT_ID, definition),
        });
      }
    );
    await mockApi.raw(page, runtimeUrl('reports/validate'), {
      valid: true,
      errors: [],
      warnings: [],
    });

    await gotoAppRoute(page, `/reports/${AUTHOR_REPORT_ID}?edit=1`);
    await page.getByLabel('Layout to edit').selectOption('review');
    await page
      .getByTestId('grid-review_root')
      .getByLabel('Add columns')
      .click();
    await page.getByRole('button', { name: 'Add stage group' }).click();
    await expect(page.getByText(/Stage navigation/)).toBeVisible();
    await page.getByRole('button', { name: /^Save$/ }).click();

    await expect.poll(() => saved).not.toBeNull();
    expect(
      saved?.definition.views?.find((candidate) => candidate.id === 'review')
        ?.layout?.columns
    ).toBe(2);
    expect(saved?.definition.viewGroups?.[0]).toMatchObject({
      mode: 'stages',
      access: 'through_current',
      currentFrom: { type: 'filter', filterId: 'stage' },
      followCurrentOnAdvance: true,
      stages: [
        { viewId: 'draft', value: 'draft' },
        { viewId: 'review', value: 'review' },
      ],
    });
  });
});
