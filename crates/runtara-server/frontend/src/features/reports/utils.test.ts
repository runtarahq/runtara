import { describe, expect, it } from 'vitest';

import {
  canonicalConditionToReportVisibility,
  canonicalToLegacyCondition,
  getReportViewBreadcrumbs,
  isWorkflowActionDisabled,
  isWorkflowActionVisible,
  legacyToCanonicalCondition,
  matchesReportRowCondition,
  reportVisibilityToCanonicalCondition,
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

describe('report visibility compatibility', () => {
  it('round-trips the persisted legacy shape through canonical conditions', () => {
    const visibility = {
      filter: 'status',
      equals: 'ready',
      notEquals: 'archived',
      exists: true,
    };

    const canonical = reportVisibilityToCanonicalCondition(visibility);
    expect(canonical).toBeDefined();
    expect(canonicalConditionToReportVisibility(canonical)).toEqual(visibility);
  });
});

describe('legacyToCanonicalCondition — condition editor shapes', () => {
  // The shared ConditionEditor emits arguments as ConditionArgument objects:
  // a reference field once picked via the variable picker, and an edited
  // immediate wrapped as {valueType:'immediate', value}. (No `type` key —
  // that's what the editor emits; the canonical helpers add `type:'value'`.)
  const editorRef = (path: string) => ({ valueType: 'reference', value: path });
  const editorImm = (value: unknown) => ({ valueType: 'immediate', value });
  const op = (name: string, args: unknown[]) => ({
    type: 'operation',
    op: name,
    arguments: args,
  });

  it('converts a field the user picked as a reference (primary bug)', () => {
    // Old bridge: typeof args[0] !== 'string' -> undefined -> VisibilityEditor
    // called onChange(undefined) -> BlockEditor deleted the block's showWhen.
    const canonical = legacyToCanonicalCondition(
      op('EQ', [editorRef('status'), editorImm('ready')]) as never
    );
    expect(canonical).toBeDefined();
    expect(matchesReportRowCondition(canonical!, { status: 'ready' })).toBe(
      true
    );
    // And it persists back as a scalar equals, not undefined.
    expect(canonicalConditionToReportVisibility(canonical)).toEqual({
      filter: 'status',
      equals: 'ready',
    });
  });

  it('does not double-wrap an editor-emitted immediate value (reverse jank)', () => {
    // Field stays a plain string (as canonicalToLegacyCondition emits it) while
    // the user edits only the value, which the editor wraps as an object.
    const canonical = legacyToCanonicalCondition(
      op('EQ', ['status', editorImm('active')]) as never
    );
    expect(canonical).toBeDefined();
    const visibility = canonicalConditionToReportVisibility(canonical);
    // Without the unwrap, equals would be the {valueType,value} object.
    expect(visibility).toEqual({ filter: 'status', equals: 'active' });
    expect(typeof visibility!.equals).toBe('string');
  });

  it('converts IN with a reference field and an immediate array', () => {
    const canonical = legacyToCanonicalCondition(
      op('IN', [editorRef('priority'), editorImm([1, 2, 3])]) as never
    );
    expect(canonical).toBeDefined();
    expect(matchesReportRowCondition(canonical!, { priority: 2 })).toBe(true);
    expect(matchesReportRowCondition(canonical!, { priority: 9 })).toBe(false);
  });

  it('converts IS_DEFINED with a reference field', () => {
    const canonical = legacyToCanonicalCondition(
      op('IS_DEFINED', [editorRef('owner')]) as never
    );
    expect(canonical).toBeDefined();
    expect(matchesReportRowCondition(canonical!, { owner: 'x' })).toBe(true);
    expect(matchesReportRowCondition(canonical!, {})).toBe(false);
  });

  it('converts an AND of mixed editor-emitted clauses', () => {
    const canonical = legacyToCanonicalCondition(
      op('AND', [
        { op: 'EQ', arguments: [editorRef('status'), editorImm('ready')] },
        { op: 'NE', arguments: ['priority', editorImm(5)] },
      ]) as never
    );
    expect(canonical).toBeDefined();
    expect(
      matchesReportRowCondition(canonical!, { status: 'ready', priority: 2 })
    ).toBe(true);
    expect(
      matchesReportRowCondition(canonical!, { status: 'ready', priority: 5 })
    ).toBe(false);
  });

  it('still rejects genuinely malformed field args', () => {
    // A nested condition where a field is expected.
    expect(
      legacyToCanonicalCondition(
        op('EQ', [
          { op: 'EQ', arguments: ['a', 'b'] },
          editorImm('x'),
        ]) as never
      )
    ).toBeUndefined();
    // A reference object whose value isn't a string.
    expect(
      legacyToCanonicalCondition(
        op('EQ', [{ valueType: 'reference', value: 123 }, editorImm('x')]) as never
      )
    ).toBeUndefined();
  });

  it('mirrors the VisibilityEditor round-trip after an edit', () => {
    // Load persisted visibility -> canonical -> legacy (what the editor shows).
    const visibility = { filter: 'status', equals: 'ready' };
    const canonical = reportVisibilityToCanonicalCondition(visibility);
    const legacy = canonicalToLegacyCondition(canonical);
    expect(legacy).toBeDefined();
    // The user edits the value; the editor re-emits it as an immediate object
    // and (after picking) the field as a reference object.
    const edited = op('EQ', [
      editorRef(String((legacy!.arguments as unknown[])[0])),
      editorImm('processed'),
    ]);
    const reconverted = legacyToCanonicalCondition(edited as never);
    expect(canonicalConditionToReportVisibility(reconverted)).toEqual({
      filter: 'status',
      equals: 'processed',
    });
  });
});

describe('report view navigation', () => {
  const reportDefinition: ReportDefinition = {
    definitionVersion: 1,
    layout: { id: 'root', columns: 1, items: [] },
    filters: [],
    blocks: [],
    views: [
      {
        id: 'a',
        title: 'Accounts',
        layout: { id: 'view_a_root', columns: 1, items: [] },
      },
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
