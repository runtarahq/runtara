import { describe, expect, it } from 'vitest';

import {
  getReportViewBreadcrumbs,
  isWorkflowActionDisabled,
  isWorkflowActionVisible,
  matchesReportRowCondition,
  truncateCellText,
} from './utils';
import type { ReportDefinition, ReportViewDefinition } from './types';

describe('report row conditions', () => {
  const row = {
    id: 'row_1',
    status: 'ready',
    priority: 2,
    owner: { team: 'risk' },
    tags: ['urgent'],
  };

  // Helpers for building canonical `ConditionExpression` shapes in tests.
  const ref = (path: string) =>
    ({ type: 'value', valueType: 'reference', value: path }) as const;
  const imm = (value: unknown) =>
    ({ type: 'value', valueType: 'immediate', value }) as const;
  const cond = (op: string, args: unknown[]) =>
    ({ type: 'operation', op, arguments: args }) as const;

  it('matches scalar and nested row fields', () => {
    expect(
      matchesReportRowCondition(
        cond('EQ', [ref('status'), imm('ready')]) as never,
        row
      )
    ).toBe(true);
    expect(
      matchesReportRowCondition(
        cond('EQ', [ref('owner.team'), imm('risk')]) as never,
        row
      )
    ).toBe(true);
    expect(
      matchesReportRowCondition(
        cond('GT', [ref('priority'), imm(1)]) as never,
        row
      )
    ).toBe(true);
  });

  it('supports logical row conditions', () => {
    expect(
      matchesReportRowCondition(
        cond('AND', [
          cond('EQ', [ref('status'), imm('ready')]),
          cond('IN', [ref('priority'), imm([1, 2, 3])]),
        ]) as never,
        row
      )
    ).toBe(true);
    expect(
      matchesReportRowCondition(
        cond('NOT', [cond('EQ', [ref('status'), imm('processed')])]) as never,
        row
      )
    ).toBe(true);
  });

  it('combines workflow action visibleWhen and hiddenWhen', () => {
    expect(
      isWorkflowActionVisible(
        {
          workflowId: 'workflow_1',
          visibleWhen: cond('EQ', [ref('status'), imm('ready')]) as never,
          hiddenWhen: cond('EQ', [ref('priority'), imm(5)]) as never,
        },
        row
      )
    ).toBe(true);
    expect(
      isWorkflowActionVisible(
        {
          workflowId: 'workflow_1',
          visibleWhen: cond('EQ', [ref('status'), imm('processed')]) as never,
        },
        row
      )
    ).toBe(false);
    expect(
      isWorkflowActionVisible(
        {
          workflowId: 'workflow_1',
          hiddenWhen: cond('EQ', [ref('status'), imm('ready')]) as never,
        },
        row
      )
    ).toBe(false);
  });

  it('evaluates workflow action disabledWhen independently from visibility', () => {
    const action = {
      workflowId: 'workflow_1',
      disabledWhen: cond('EQ', [ref('status'), imm('ready')]) as never,
    };

    expect(isWorkflowActionVisible(action, row)).toBe(true);
    expect(isWorkflowActionDisabled(action, row)).toBe(true);
  });
});

describe('report view navigation', () => {
  const reportDefinition: ReportDefinition = {
    definitionVersion: 1,
    layout: { id: 'root', columns: 1, items: [] },
    filters: [],
    blocks: [],
    views: [
      { id: 'a', title: 'Accounts', layout: { id: 'view_a_root', columns: 1, items: [] } },
      {
        id: 'b',
        title: 'Branches',
        parentViewId: 'a',
        clearFiltersOnBack: ['b_id'],
        layout: { id: 'view_b_root', columns: 1, items: [] },
      },
      {
        id: 'c',
        title: 'Cases',
        parentViewId: 'b',
        clearFiltersOnBack: ['c_id'],
        layout: { id: 'view_c_root', columns: 1, items: [] },
      },
    ],
  };

  const labelForView = (view: ReportViewDefinition) => view.title ?? view.id;

  it('builds breadcrumbs from parent views and accumulated back filters', () => {
    const view = reportDefinition.views?.find((view) => view.id === 'c');
    expect(view).toBeDefined();

    expect(
      getReportViewBreadcrumbs(
        reportDefinition,
        view as ReportViewDefinition,
        labelForView
      )
    ).toEqual([
      {
        label: 'Accounts',
        viewId: 'a',
        clearFilters: ['b_id', 'c_id'],
      },
      {
        label: 'Branches',
        viewId: 'b',
        clearFilters: ['c_id'],
      },
    ]);
  });

  it('keeps manual breadcrumbs as an explicit override', () => {
    const view: ReportViewDefinition = {
      id: 'c',
      parentViewId: 'b',
      clearFiltersOnBack: ['c_id'],
      breadcrumb: [
        {
          label: 'Custom',
          viewId: 'a',
          clearFilters: ['custom_id'],
        },
      ],
    };

    expect(
      getReportViewBreadcrumbs(reportDefinition, view, labelForView)
    ).toEqual(view.breadcrumb);
  });
});

describe('truncateCellText', () => {
  it('keeps full text when maxChars is omitted or non-positive', () => {
    expect(truncateCellText('Hanes Mens Double Tough Socks')).toEqual({
      text: 'Hanes Mens Double Tough Socks',
    });
    expect(truncateCellText('Hanes Mens Double Tough Socks', 0)).toEqual({
      text: 'Hanes Mens Double Tough Socks',
    });
  });

  it('cuts displayed text after the configured number of characters', () => {
    expect(truncateCellText('Hanes Mens Double Tough Socks', 12)).toEqual({
      text: 'Hanes Mens D...',
      title: 'Hanes Mens Double Tough Socks',
    });
  });

  it('counts unicode code points instead of UTF-16 halves', () => {
    expect(truncateCellText('A😀BC', 2)).toEqual({
      text: 'A😀...',
      title: 'A😀BC',
    });
  });
});

// `renderDisplayTemplate` is now backed by the WASM minijinja engine in
// `runtara-report-dsl`. End-to-end behavior is covered by the Rust tests in
// `runtara-report-dsl/src/template.rs` and the Playwright report-corpus suite.
// Vitest doesn't load the WASM bundle, so we keep no FE-side template tests.
