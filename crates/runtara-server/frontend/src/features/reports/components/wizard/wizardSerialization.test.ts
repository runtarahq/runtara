import { describe, expect, it } from 'vitest';
import {
  definitionToWizardState,
  wizardStateToDefinition,
} from './wizardSerialization';
import type { ReportDefinition } from '../../types';

const SCHEMA_FIELDS: Record<string, string[]> = {
  orders: ['id', 'status', 'amount'],
};

describe('table column round-trip', () => {
  it('preserves workflow_button columns, interaction_buttons columns, selectable, and bulk actions', () => {
    const definition: ReportDefinition = {
      definitionVersion: 1,
      filters: [],
      blocks: [
        {
          id: 'orders',
          type: 'table',
          title: 'Orders',
          source: { schema: 'orders', mode: 'filter' },
          table: {
            columns: [
              { field: 'id', label: 'Id' },
              { field: 'status', label: 'Status', format: 'pill' },
              {
                field: 'process',
                label: 'Process',
                type: 'workflow_button',
                workflowAction: {
                  workflowId: 'workflow_process',
                  label: 'Process',
                  reloadBlock: true,
                  context: { mode: 'row' },
                },
              },
              {
                field: 'open',
                label: 'Open',
                type: 'interaction_buttons',
                interactionButtons: [
                  {
                    id: 'open_detail',
                    label: 'Open',
                    icon: 'arrow_right',
                    actions: [
                      {
                        type: 'set_filter',
                        filterId: 'order_id',
                        valueFrom: 'datum.id',
                      },
                    ],
                  },
                ],
              },
            ],
            selectable: true,
            actions: [
              {
                id: 'bulk_archive',
                label: 'Archive selected',
                workflowAction: {
                  workflowId: 'workflow_archive',
                  label: 'Archive',
                  reloadBlock: true,
                  context: { mode: 'selection' },
                },
              },
            ],
          },
        },
      ],
    };

    const { state, compatibility } = definitionToWizardState(
      definition,
      'orders'
    );
    expect(compatibility.fullyEditable).toBe(true);
    expect(compatibility.reasons).toEqual([]);

    const round = wizardStateToDefinition(state, SCHEMA_FIELDS, definition);
    const table = round.blocks[0].table!;
    expect(table.selectable).toBe(true);
    expect(table.actions).toHaveLength(1);
    expect(table.actions?.[0].id).toBe('bulk_archive');
    expect(table.actions?.[0].workflowAction.workflowId).toBe(
      'workflow_archive'
    );
    expect(table.actions?.[0].workflowAction.context?.mode).toBe('selection');

    const cols = table.columns ?? [];
    expect(cols.map((c) => c.type ?? 'value')).toEqual([
      'value',
      'value',
      'workflow_button',
      'interaction_buttons',
    ]);
    const wfCol = cols.find((c) => c.type === 'workflow_button')!;
    expect(wfCol.workflowAction?.workflowId).toBe('workflow_process');
    expect(wfCol.workflowAction?.context?.mode).toBe('row');
    const ixCol = cols.find((c) => c.type === 'interaction_buttons')!;
    expect(ixCol.interactionButtons?.[0].actions[0]).toMatchObject({
      type: 'set_filter',
      filterId: 'order_id',
      valueFrom: 'datum.id',
    });
  });

  it('round-trips datasets and per-block dataset queries without losing them', () => {
    const definition: ReportDefinition = {
      definitionVersion: 1,
      filters: [],
      datasets: [
        {
          id: 'order_totals',
          label: 'Order totals',
          source: { schema: 'orders' },
          dimensions: [
            { field: 'status', label: 'Status', type: 'string' },
          ],
          measures: [
            {
              id: 'total_amount',
              label: 'Total',
              op: 'sum',
              field: 'amount',
              format: 'currency',
            },
            {
              id: 'count',
              label: 'Count',
              op: 'count',
              format: 'number',
            },
          ],
        },
      ],
      blocks: [
        {
          id: 'totals_table',
          type: 'table',
          title: 'Totals',
          source: { schema: '' },
          dataset: {
            id: 'order_totals',
            dimensions: ['status'],
            measures: ['total_amount'],
            orderBy: [{ field: 'total_amount', direction: 'desc' }],
            limit: 50,
          },
        },
      ],
    };

    const { state, compatibility } = definitionToWizardState(
      definition,
      'orders'
    );
    expect(compatibility.fullyEditable).toBe(true);
    expect(state.datasets).toHaveLength(1);
    expect(state.blocks[0].dataset?.id).toBe('order_totals');

    const round = wizardStateToDefinition(state, SCHEMA_FIELDS, definition);
    expect(round.datasets).toHaveLength(1);
    expect(round.datasets?.[0].measures[0].field).toBe('amount');
    const block = round.blocks[0];
    expect(block.dataset?.dimensions).toEqual(['status']);
    expect(block.dataset?.measures).toEqual(['total_amount']);
    // reconcileDatasetBlock builds the table columns from the query output.
    expect(block.table?.columns?.map((c) => c.field)).toEqual([
      'status',
      'total_amount',
    ]);
  });

  it('does not flag the new features as advanced (compatibility banner stays hidden)', () => {
    const definition: ReportDefinition = {
      definitionVersion: 1,
      filters: [],
      blocks: [
        {
          id: 'tasks',
          type: 'table',
          title: 'Tasks',
          source: { schema: 'orders', mode: 'filter' },
          table: {
            selectable: true,
            actions: [
              {
                id: 'bulk_run',
                label: 'Run on selection',
                workflowAction: {
                  workflowId: 'workflow_run',
                  context: { mode: 'selection' },
                },
              },
            ],
            columns: [
              { field: 'id' },
              {
                field: 'run',
                type: 'workflow_button',
                workflowAction: { workflowId: 'workflow_x' },
              },
            ],
          },
        },
      ],
    };

    const { compatibility } = definitionToWizardState(definition, 'orders');
    // None of these new-features should appear as a reason in the warning list.
    for (const reason of compatibility.reasons) {
      expect(reason).not.toMatch(/Selectable rows/i);
      expect(reason).not.toMatch(/Table workflow actions/i);
      expect(reason).not.toMatch(/Workflow buttons in tables/i);
      expect(reason).not.toMatch(/Row interaction buttons/i);
      expect(reason).not.toMatch(/Column type "workflow_button"/i);
      expect(reason).not.toMatch(/Column type "interaction_buttons"/i);
    }
  });
});
