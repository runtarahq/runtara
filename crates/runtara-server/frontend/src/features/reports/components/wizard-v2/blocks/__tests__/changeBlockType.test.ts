import { describe, expect, it } from 'vitest';
import {
  changeBlockType,
  hasMeaningfulTypeConfig,
} from '../changeBlockType';
import { ReportBlockDefinition } from '../../../../types';

function tableBlock(): ReportBlockDefinition {
  return {
    id: 'b1',
    type: 'table',
    title: 'Recent orders',
    source: { schema: 'Order', mode: 'filter' },
    table: {
      columns: [
        { field: 'id', label: 'Id' },
        { field: 'status', label: 'Status' },
      ],
    },
  } as ReportBlockDefinition;
}

function markdownBlock(): ReportBlockDefinition {
  return {
    id: 'b1',
    type: 'markdown',
    title: 'Intro',
    source: { schema: '' },
    markdown: { content: '# Hi' },
  } as ReportBlockDefinition;
}

function emptyChartBlock(): ReportBlockDefinition {
  return {
    id: 'b1',
    type: 'chart',
    title: null,
    source: { schema: 'Order', mode: 'aggregate' },
    chart: { kind: 'bar', x: '', series: [] },
  } as ReportBlockDefinition;
}

function actionsBlock(): ReportBlockDefinition {
  return {
    id: 'b1',
    type: 'actions',
    source: {
      kind: 'workflow_runtime',
      schema: '',
      entity: 'actions',
      workflowId: 'inventory_sync',
      mode: 'filter',
    },
    actions: { submit: {} },
  } as ReportBlockDefinition;
}

describe('changeBlockType', () => {
  it('returns the block unchanged when newType matches', () => {
    const before = tableBlock();
    const after = changeBlockType(before, 'table');
    expect(after).toBe(before);
  });

  it('preserves id, title, and source.schema across a simple type swap', () => {
    const before = tableBlock();
    const after = changeBlockType(before, 'chart');
    expect(after.id).toBe(before.id);
    expect(after.title).toBe(before.title);
    // Source schema + kind are preserved across type swap; switching to
    // chart flips mode to aggregate and seeds a default aggregate (the
    // server requires at least one for chart/metric renders).
    expect(after.source.schema).toBe(before.source.schema);
    expect(after.source.kind).toBe(before.source.kind);
  });

  it('switching markdown → table leaves source untouched (no aggregate seeding)', () => {
    const before = markdownBlock();
    const after = changeBlockType(before, 'table');
    expect(after.source).toEqual(before.source);
  });

  it('drops the previous type-specific config field', () => {
    const before = tableBlock();
    const after = changeBlockType(before, 'chart') as ReportBlockDefinition & {
      table?: unknown;
    };
    expect(after.type).toBe('chart');
    expect(after.table).toBeUndefined();
  });

  it('seeds the new type with its default config shape', () => {
    const before = tableBlock();
    const after = changeBlockType(before, 'metric') as ReportBlockDefinition & {
      metric?: { valueField?: string };
    };
    expect(after.type).toBe('metric');
    // Phase 11 follow-up: switching to metric seeds a `count(*)`
    // aggregate on the source and wires metric.valueField to it so
    // the block renders out of the box.
    expect(after.metric).toEqual({ valueField: 'value' });
    expect(after.source.mode).toBe('aggregate');
    expect(after.source.aggregates).toEqual([
      { alias: 'value', op: 'count' },
    ]);
  });

  it('switching to chart seeds a default aggregate + series binding', () => {
    const before = markdownBlock();
    const after = changeBlockType(before, 'chart') as ReportBlockDefinition & {
      chart?: { series?: Array<{ field: string }> };
    };
    expect(after.type).toBe('chart');
    expect(after.source.mode).toBe('aggregate');
    expect(after.source.aggregates).toEqual([
      { alias: 'value', op: 'count' },
    ]);
    expect(after.chart?.series).toEqual([{ field: 'value' }]);
  });

  it('switching TO actions resets source to workflow_runtime', () => {
    const before = tableBlock();
    const after = changeBlockType(before, 'actions');
    expect(after.source.kind).toBe('workflow_runtime');
    expect(after.source.entity).toBe('actions');
    expect(after.source.mode).toBe('filter');
  });

  it('switching FROM actions resets source to object_model', () => {
    const before = actionsBlock();
    const after = changeBlockType(before, 'markdown');
    expect(after.source.kind).toBe('object_model');
    // actions field is gone, markdown is seeded.
    const cast = after as ReportBlockDefinition & {
      actions?: unknown;
      markdown?: { content?: string };
    };
    expect(cast.actions).toBeUndefined();
    expect(cast.markdown).toEqual({ content: '' });
  });

  it('switching TO file_upload resets the source and seeds a value-mode action', () => {
    const before = tableBlock();
    const after = changeBlockType(before, 'file_upload') as ReportBlockDefinition & {
      table?: unknown;
    };
    expect(after.type).toBe('file_upload');
    expect(after.table).toBeUndefined();
    // file_upload blocks reject any source — the switch must clear it.
    expect(after.source).toEqual({
      kind: 'object_model',
      schema: '',
      mode: 'filter',
    });
    expect(after.file_upload).toEqual({
      trigger: 'button',
      workflowAction: {
        id: 'upload',
        workflowId: '',
        label: 'Run workflow',
        reloadBlock: true,
        context: { mode: 'value', inputKey: 'file' },
      },
    });
  });
});

describe('hasMeaningfulTypeConfig', () => {
  it('returns true for a table with columns', () => {
    expect(hasMeaningfulTypeConfig(tableBlock())).toBe(true);
  });

  it('returns true for a markdown with content', () => {
    expect(hasMeaningfulTypeConfig(markdownBlock())).toBe(true);
  });

  it('returns false for an empty chart block', () => {
    expect(hasMeaningfulTypeConfig(emptyChartBlock())).toBe(false);
  });

  it('returns false for a freshly seeded block (no user data)', () => {
    // Mimic the just-created state from handleAddBlockToGrid.
    const fresh: ReportBlockDefinition = {
      id: 'new',
      type: 'markdown',
      title: 'New block',
      source: { schema: '' },
      markdown: { content: '' },
    } as ReportBlockDefinition;
    expect(hasMeaningfulTypeConfig(fresh)).toBe(false);
  });

  it('returns true for an actions block with a workflowId on the source', () => {
    expect(hasMeaningfulTypeConfig(actionsBlock())).toBe(true);
  });

  it('file_upload is meaningful only once a workflow is picked', () => {
    const fresh: ReportBlockDefinition = {
      id: 'b1',
      type: 'file_upload',
      source: { schema: '' },
      file_upload: {
        workflowAction: {
          id: 'upload',
          workflowId: '',
          context: { mode: 'value', inputKey: 'file' },
        },
      },
    } as ReportBlockDefinition;
    expect(hasMeaningfulTypeConfig(fresh)).toBe(false);

    const configured = {
      ...fresh,
      file_upload: {
        workflowAction: {
          id: 'upload',
          workflowId: 'import_prices',
          context: { mode: 'value', inputKey: 'file' },
        },
      },
    } as ReportBlockDefinition;
    expect(hasMeaningfulTypeConfig(configured)).toBe(true);
  });
});
