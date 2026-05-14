import { describe, expect, it } from 'vitest';

import {
  compileDisplayTemplate,
  getReportViewBreadcrumbs,
  isWorkflowActionDisabled,
  isWorkflowActionVisible,
  matchesReportRowCondition,
  renderDisplayTemplate,
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

  it('matches scalar and nested row fields', () => {
    expect(
      matchesReportRowCondition(
        { op: 'EQ', arguments: ['status', 'ready'] },
        row
      )
    ).toBe(true);
    expect(
      matchesReportRowCondition(
        { op: 'EQ', arguments: ['owner.team', 'risk'] },
        row
      )
    ).toBe(true);
    expect(
      matchesReportRowCondition({ op: 'GT', arguments: ['priority', 1] }, row)
    ).toBe(true);
  });

  it('supports logical row conditions', () => {
    expect(
      matchesReportRowCondition(
        {
          op: 'AND',
          arguments: [
            { op: 'EQ', arguments: ['status', 'ready'] },
            { op: 'IN', arguments: ['priority', [1, 2, 3]] },
          ],
        },
        row
      )
    ).toBe(true);
    expect(
      matchesReportRowCondition(
        {
          op: 'NOT',
          arguments: [{ op: 'EQ', arguments: ['status', 'processed'] }],
        },
        row
      )
    ).toBe(true);
  });

  it('combines workflow action visibleWhen and hiddenWhen', () => {
    expect(
      isWorkflowActionVisible(
        {
          workflowId: 'workflow_1',
          visibleWhen: { op: 'EQ', arguments: ['status', 'ready'] },
          hiddenWhen: { op: 'EQ', arguments: ['priority', 5] },
        },
        row
      )
    ).toBe(true);
    expect(
      isWorkflowActionVisible(
        {
          workflowId: 'workflow_1',
          visibleWhen: { op: 'EQ', arguments: ['status', 'processed'] },
        },
        row
      )
    ).toBe(false);
    expect(
      isWorkflowActionVisible(
        {
          workflowId: 'workflow_1',
          hiddenWhen: { op: 'EQ', arguments: ['status', 'ready'] },
        },
        row
      )
    ).toBe(false);
  });

  it('evaluates workflow action disabledWhen independently from visibility', () => {
    const action = {
      workflowId: 'workflow_1',
      disabledWhen: { op: 'EQ', arguments: ['status', 'ready'] },
    };

    expect(isWorkflowActionVisible(action, row)).toBe(true);
    expect(isWorkflowActionDisabled(action, row)).toBe(true);
  });
});

describe('report view navigation', () => {
  const reportDefinition: ReportDefinition = {
    definitionVersion: 1,
    filters: [],
    blocks: [],
    views: [
      { id: 'a', title: 'Accounts' },
      {
        id: 'b',
        title: 'Branches',
        parentViewId: 'a',
        clearFiltersOnBack: ['b_id'],
      },
      {
        id: 'c',
        title: 'Cases',
        parentViewId: 'b',
        clearFiltersOnBack: ['c_id'],
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

describe('renderDisplayTemplate', () => {
  const row = {
    first_name: 'Jane',
    last_name: 'Doe',
    applicant_summary: {
      full_name: 'Jane Q Doe',
    },
    requested_loan: {
      amount: 120000,
    },
  };

  it('concatenates row fields for display-only cells', () => {
    expect(compileDisplayTemplate('{{first_name}} {{last_name}}')).toEqual({
      parts: [
        { kind: 'placeholder', field: 'first_name' },
        { kind: 'literal', value: ' ' },
        { kind: 'placeholder', field: 'last_name' },
      ],
    });
    expect(renderDisplayTemplate(row, '{{first_name}} {{last_name}}')).toBe(
      'Jane Doe'
    );
  });

  it('reads nested JSON paths and applies token formats', () => {
    expect(renderDisplayTemplate(row, '{{applicant_summary.full_name}}')).toBe(
      'Jane Q Doe'
    );

    const rendered = renderDisplayTemplate(
      row,
      '${{requested_loan.amount | number_compact}} AUD'
    );
    expect(rendered).toMatch(/^\$.+ AUD$/);
    expect(rendered).not.toContain('{{');
  });

  it('only compiles safe variable interpolation tokens', () => {
    expect(() => compileDisplayTemplate('{{#if status}}')).toThrow();
    expect(() =>
      compileDisplayTemplate('{{first_name + last_name}}')
    ).toThrow();
    expect(() => compileDisplayTemplate('{{first_name | }}')).toThrow();
    expect(() => compileDisplayTemplate('{{first_name')).toThrow();
    expect(renderDisplayTemplate(row, '{{first_name + last_name}}')).toBe('');
  });
});
