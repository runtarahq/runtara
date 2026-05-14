import { describe, expect, it } from 'vitest';
import {
  definitionToWizardState,
  wizardStateToDefinition,
} from './wizardSerialization';
import { WIZARD_FILTER_TARGET_CUSTOM } from './wizardTypes';
import type { ReportDefinition } from '../../types';

const SCHEMA_FIELDS: Record<string, string[]> = {
  orders: ['id', 'status', 'amount'],
};

describe('table column round-trip', () => {
  it('loads markdown blocks that omit source', () => {
    const definition = {
      definitionVersion: 1,
      filters: [],
      layout: [
        {
          type: 'section',
          id: 'overview',
          children: [
            { type: 'block', id: 'intro_node', blockId: 'intro' },
            { type: 'block', id: 'orders_node', blockId: 'orders' },
          ],
        },
      ],
      blocks: [
        {
          id: 'intro',
          type: 'markdown',
          markdown: { content: '# Intro' },
        },
        {
          id: 'orders',
          type: 'table',
          title: 'Orders',
          source: { schema: 'orders', mode: 'filter' },
          table: {
            columns: [{ field: 'status', label: 'Status' }],
          },
        },
      ],
    } as unknown as ReportDefinition;

    const { state, compatibility } = definitionToWizardState(
      definition,
      'fallback'
    );

    expect(compatibility.fullyEditable).toBe(true);
    expect(compatibility.reasons).toEqual([]);
    expect(state.defaultSchema).toBe('orders');
    expect(state.blocks[0]).toMatchObject({
      id: 'intro',
      type: 'markdown',
      markdownContent: '# Intro',
    });
    expect(state.blocks[0].schema).toBeUndefined();
  });

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

  it('infers per-row action columns from workflow actions and interaction buttons', () => {
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
              {
                field: 'run',
                label: 'Run',
                workflowAction: {
                  workflowId: 'workflow_run',
                  label: 'Run',
                  context: { mode: 'row' },
                },
              },
              {
                field: 'open',
                label: 'Open',
                interactionButtons: [
                  {
                    id: 'open_detail',
                    label: 'Open',
                    actions: [
                      {
                        type: 'navigate_view',
                        viewId: 'detail',
                      },
                    ],
                  },
                ],
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
    expect(state.blocks[0].fieldConfigs?.run).toMatchObject({
      columnType: 'workflow_button',
      workflowAction: definition.blocks[0].table?.columns?.[1].workflowAction,
    });
    expect(state.blocks[0].fieldConfigs?.open).toMatchObject({
      columnType: 'interaction_buttons',
      interactionButtons:
        definition.blocks[0].table?.columns?.[2].interactionButtons,
    });

    const round = wizardStateToDefinition(state, SCHEMA_FIELDS, definition);
    expect(round.blocks[0].table?.columns?.[1]).toMatchObject({
      type: 'workflow_button',
      workflowAction: definition.blocks[0].table?.columns?.[1].workflowAction,
    });
    expect(round.blocks[0].table?.columns?.[2]).toMatchObject({
      type: 'interaction_buttons',
      interactionButtons:
        definition.blocks[0].table?.columns?.[2].interactionButtons,
    });
  });

  it('round-trips table pagination and default sort settings', () => {
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
              { field: 'status', label: 'Status' },
              { field: 'amount', label: 'Amount', format: 'currency' },
            ],
            defaultSort: [{ field: 'amount', direction: 'desc' }],
            pagination: {
              defaultPageSize: 25,
              allowedPageSizes: [10, 25, 100],
            },
          },
        },
      ],
    };

    const { state, compatibility } = definitionToWizardState(
      definition,
      'orders'
    );
    expect(compatibility.fullyEditable).toBe(true);
    expect(state.blocks[0].defaultSort).toEqual([
      { field: 'amount', direction: 'desc' },
    ]);
    expect(state.blocks[0].defaultPageSize).toBe(25);
    expect(state.blocks[0].allowedPageSizes).toEqual([10, 25, 100]);

    const round = wizardStateToDefinition(state, SCHEMA_FIELDS, definition);
    expect(round.blocks[0].table?.defaultSort).toEqual([
      { field: 'amount', direction: 'desc' },
    ]);
    expect(round.blocks[0].table?.pagination).toEqual({
      defaultPageSize: 25,
      allowedPageSizes: [10, 25, 100],
    });
  });

  it('round-trips block visibility and lazy loading settings', () => {
    const definition: ReportDefinition = {
      definitionVersion: 1,
      filters: [
        {
          id: 'status_filter',
          label: 'Status',
          type: 'select',
          appliesTo: [{ field: 'status', op: 'eq' }],
        },
      ],
      blocks: [
        {
          id: 'orders',
          type: 'table',
          title: 'Orders',
          lazy: true,
          hideWhenEmpty: true,
          showWhen: { filter: 'status_filter', equals: 'open' },
          source: { schema: 'orders', mode: 'filter' },
          table: {
            columns: [{ field: 'status', label: 'Status' }],
          },
        },
      ],
    };

    const { state, compatibility } = definitionToWizardState(
      definition,
      'orders'
    );
    expect(compatibility.fullyEditable).toBe(true);
    expect(state.blocks[0]).toMatchObject({
      lazy: true,
      hideWhenEmpty: true,
      showWhen: { filter: 'status_filter', equals: 'open' },
    });

    const round = wizardStateToDefinition(state, SCHEMA_FIELDS, definition);
    expect(round.blocks[0]).toMatchObject({
      lazy: true,
      hideWhenEmpty: true,
      showWhen: { filter: 'status_filter', equals: 'open' },
    });
  });

  it('round-trips block-level filters without compatibility warning', () => {
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
              { field: 'id', label: 'ID' },
              { field: 'status', label: 'Status' },
            ],
          },
          filters: [
            {
              id: 'status_filter',
              label: 'Status',
              type: 'select',
              appliesTo: [
                {
                  blockId: 'orders',
                  field: 'status',
                  op: 'eq',
                },
              ],
              options: {
                source: 'static',
                values: [
                  { label: 'Open', value: 'open' },
                  { label: 'Closed', value: 'closed' },
                ],
              },
            },
          ],
        },
      ],
    };

    const { state, compatibility } = definitionToWizardState(
      definition,
      'orders'
    );
    expect(compatibility.fullyEditable).toBe(true);
    expect(state.blocks[0].filters).toEqual(definition.blocks[0].filters);

    const round = wizardStateToDefinition(state, SCHEMA_FIELDS, definition);
    expect(round.blocks[0].filters).toEqual(definition.blocks[0].filters);
  });

  it('round-trips editable field writeback editor configs', () => {
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
              {
                field: 'status',
                label: 'Status',
                format: 'pill',
                editable: true,
                editor: {
                  kind: 'select',
                  options: [
                    { label: 'Open', value: 'open' },
                    { label: 'Closed', value: 'closed' },
                  ],
                },
              },
            ],
          },
        },
        {
          id: 'order_card',
          type: 'card',
          title: 'Order',
          source: { schema: 'orders', mode: 'filter' },
          card: {
            groups: [
              {
                id: 'main',
                fields: [
                  {
                    field: 'amount',
                    label: 'Amount',
                    editable: true,
                    editor: {
                      kind: 'number',
                      min: 0,
                      step: 0.01,
                    },
                  },
                ],
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
    expect(state.blocks[0].fieldConfigs?.status).toMatchObject({
      editable: true,
      editor: definition.blocks[0].table?.columns?.[0].editor,
    });
    expect(state.blocks[1].fieldConfigs?.amount).toMatchObject({
      editable: true,
      editor: definition.blocks[1].card?.groups[0].fields[0].editor,
    });

    const round = wizardStateToDefinition(state, SCHEMA_FIELDS, definition);
    expect(round.blocks[0].table?.columns?.[0]).toMatchObject({
      editable: true,
      editor: definition.blocks[0].table?.columns?.[0].editor,
    });
    expect(round.blocks[1].card?.groups[0].fields[0]).toMatchObject({
      editable: true,
      editor: definition.blocks[1].card?.groups[0].fields[0].editor,
    });
  });

  it('round-trips table column display modifiers', () => {
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
              {
                field: 'customer_id',
                label: 'Customer',
                displayField: 'customer_name',
                displayTemplate: '{{customer_name}} (#{{customer_id}})',
                secondaryField: 'customer_email',
                linkField: 'customer_url',
                tooltipField: 'customer_email',
                align: 'center',
                maxChars: 24,
                descriptive: true,
              },
              {
                field: 'status',
                label: 'Status',
                format: 'bar_indicator',
                levels: ['queued', 'open', 'closed'],
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
    expect(state.blocks[0].fieldConfigs?.customer_id).toMatchObject({
      displayField: 'customer_name',
      displayTemplate: '{{customer_name}} (#{{customer_id}})',
      secondaryField: 'customer_email',
      linkField: 'customer_url',
      tooltipField: 'customer_email',
      align: 'center',
      maxChars: 24,
      descriptive: true,
    });
    expect(state.blocks[0].fieldConfigs?.status?.levels).toEqual([
      'queued',
      'open',
      'closed',
    ]);

    const round = wizardStateToDefinition(state, SCHEMA_FIELDS, definition);
    expect(round.blocks[0].table?.columns?.[0]).toMatchObject({
      displayField: 'customer_name',
      displayTemplate: '{{customer_name}} (#{{customer_id}})',
      secondaryField: 'customer_email',
      linkField: 'customer_url',
      tooltipField: 'customer_email',
      align: 'center',
      maxChars: 24,
      descriptive: true,
    });
    expect(round.blocks[0].table?.columns?.[1].levels).toEqual([
      'queued',
      'open',
      'closed',
    ]);
  });

  it('round-trips workflow runtime and system source metadata', () => {
    const definition: ReportDefinition = {
      definitionVersion: 1,
      filters: [],
      blocks: [
        {
          id: 'runs',
          type: 'table',
          title: 'Workflow runs',
          source: {
            kind: 'workflow_runtime',
            schema: '',
            entity: 'instances',
            workflowId: 'inventory_sync',
            mode: 'filter',
          },
          table: {
            columns: [
              { field: 'instanceId', label: 'Instance' },
              { field: 'status', label: 'Status', format: 'pill' },
            ],
          },
        },
        {
          id: 'rate_limit_timeline',
          type: 'chart',
          title: 'Rate limit timeline',
          source: {
            kind: 'system',
            schema: '',
            entity: 'connection_rate_limit_timeline',
            mode: 'aggregate',
            granularity: 'hourly',
            interval: '24h',
            groupBy: ['bucketTime'],
            aggregates: [{ alias: 'value', op: 'count' }],
          },
          chart: {
            kind: 'bar',
            x: 'bucketTime',
            series: [{ field: 'value' }],
          },
        },
      ],
    };

    const { state, compatibility } = definitionToWizardState(
      definition,
      'orders'
    );
    expect(compatibility.fullyEditable).toBe(true);
    expect(state.blocks[0]).toMatchObject({
      sourceKind: 'workflow_runtime',
      sourceEntity: 'instances',
      workflowId: 'inventory_sync',
    });
    expect(state.blocks[1]).toMatchObject({
      sourceKind: 'system',
      sourceEntity: 'connection_rate_limit_timeline',
      sourceGranularity: 'hourly',
      sourceInterval: '24h',
    });

    const round = wizardStateToDefinition(state, SCHEMA_FIELDS, definition);
    expect(round.blocks[0].source).toMatchObject({
      kind: 'workflow_runtime',
      schema: '',
      entity: 'instances',
      workflowId: 'inventory_sync',
      mode: 'filter',
    });
    expect(round.blocks[1].source).toMatchObject({
      kind: 'system',
      schema: '',
      entity: 'connection_rate_limit_timeline',
      granularity: 'hourly',
      interval: '24h',
      mode: 'aggregate',
      groupBy: ['bucketTime'],
    });
  });

  it('round-trips schema joins and custom source conditions', () => {
    const condition = {
      op: 'EQ',
      arguments: ['status', 'open'],
    };
    const definition: ReportDefinition = {
      definitionVersion: 1,
      filters: [],
      blocks: [
        {
          id: 'orders',
          type: 'table',
          title: 'Orders',
          source: {
            schema: 'orders',
            mode: 'filter',
            join: [
              {
                schema: 'customers',
                alias: 'customer',
                parentField: 'customer_id',
                field: 'id',
                op: 'eq',
                kind: 'left',
              },
            ],
            condition,
          },
          table: {
            columns: [
              { field: 'id', label: 'ID' },
              { field: 'customer.name', label: 'Customer' },
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
    expect(state.blocks[0].sourceJoins).toEqual(
      definition.blocks[0].source.join
    );
    expect(state.blocks[0].sourceCondition).toEqual(condition);

    const round = wizardStateToDefinition(
      state,
      { ...SCHEMA_FIELDS, customers: ['id', 'name'] },
      definition
    );
    expect(round.blocks[0].source.join).toEqual(
      definition.blocks[0].source.join
    );
    expect(round.blocks[0].source.condition).toEqual(condition);
  });

  it('round-trips source order, limit, interval, and granularity', () => {
    const definition: ReportDefinition = {
      definitionVersion: 1,
      filters: [],
      blocks: [
        {
          id: 'orders',
          type: 'table',
          title: 'Orders',
          source: {
            schema: 'orders',
            mode: 'filter',
            orderBy: [{ field: 'amount', direction: 'desc' }],
            limit: 25,
            interval: '30d',
            granularity: 'daily',
          },
          table: {
            columns: [
              { field: 'id', label: 'ID' },
              { field: 'amount', label: 'Amount' },
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
    expect(state.blocks[0]).toMatchObject({
      sourceOrderBy: [{ field: 'amount', direction: 'desc' }],
      sourceLimit: 25,
      sourceInterval: '30d',
      sourceGranularity: 'daily',
    });

    const round = wizardStateToDefinition(state, SCHEMA_FIELDS, definition);
    expect(round.blocks[0].source).toMatchObject({
      orderBy: [{ field: 'amount', direction: 'desc' }],
      limit: 25,
      interval: '30d',
      granularity: 'daily',
    });
  });

  it('round-trips advanced metric and chart aggregate specs', () => {
    const definition: ReportDefinition = {
      definitionVersion: 1,
      filters: [],
      blocks: [
        {
          id: 'p90_amount',
          type: 'metric',
          title: 'P90 amount',
          source: {
            schema: 'orders',
            mode: 'aggregate',
            aggregates: [
              {
                alias: 'value',
                op: 'percentile_cont',
                field: 'amount',
                percentile: 0.9,
                distinct: true,
              },
            ],
          },
          metric: {
            valueField: 'value',
            label: 'P90 amount',
            format: 'currency',
          },
        },
        {
          id: 'status_total',
          type: 'chart',
          title: 'Status total',
          source: {
            schema: 'orders',
            mode: 'aggregate',
            groupBy: ['status'],
            aggregates: [
              {
                alias: 'value',
                op: 'sum',
                field: 'amount',
              },
            ],
          },
          chart: {
            kind: 'bar',
            x: 'status',
            series: [{ field: 'value', label: 'Total' }],
          },
        },
      ],
    };

    const { state, compatibility } = definitionToWizardState(
      definition,
      'orders'
    );
    expect(compatibility.fullyEditable).toBe(true);
    expect(state.blocks[0]).toMatchObject({
      metricAggregate: 'percentile_cont',
      metricField: 'amount',
      metricPercentile: 0.9,
      metricDistinct: true,
    });
    expect(state.blocks[1]).toMatchObject({
      chartAggregate: 'sum',
      chartAggregateField: 'amount',
    });

    const round = wizardStateToDefinition(state, SCHEMA_FIELDS, definition);
    expect(round.blocks[0].source.aggregates?.[0]).toMatchObject({
      alias: 'value',
      op: 'percentile_cont',
      field: 'amount',
      percentile: 0.9,
      distinct: true,
    });
    expect(round.blocks[1].source.aggregates?.[0]).toMatchObject({
      alias: 'value',
      op: 'sum',
      field: 'amount',
    });
  });

  it('round-trips advanced card configs without compatibility warning', () => {
    const definition: ReportDefinition = {
      definitionVersion: 1,
      filters: [],
      blocks: [
        {
          id: 'order_card',
          type: 'card',
          title: 'Order card',
          source: { schema: 'orders', mode: 'filter' },
          card: {
            groups: [
              {
                id: 'summary',
                title: 'Summary',
                fields: [
                  { field: 'status', label: 'Status', format: 'pill' },
                  {
                    field: 'notes',
                    label: 'Notes',
                    kind: 'markdown',
                    collapsed: true,
                  },
                ],
              },
              {
                id: 'lines',
                title: 'Lines',
                fields: [
                  {
                    field: 'line_items',
                    label: 'Line items',
                    kind: 'subtable',
                    subtable: {
                      columns: [
                        { field: 'sku', label: 'SKU' },
                        { field: 'quantity', label: 'Qty', format: 'number' },
                      ],
                    },
                  },
                ],
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
    expect(state.blocks[0].cardConfig).toEqual(definition.blocks[0].card);

    const round = wizardStateToDefinition(state, SCHEMA_FIELDS, definition);
    expect(round.blocks[0].card).toEqual(definition.blocks[0].card);
  });

  it('round-trips advanced filter option metadata', () => {
    const filterMappings = [
      { filterId: 'country_filter', field: 'country_id', op: 'eq' },
    ];
    const optionsCondition = {
      op: 'EQ',
      arguments: ['active', true],
    };
    const definition: ReportDefinition = {
      definitionVersion: 1,
      filters: [
        {
          id: 'country_filter',
          label: 'Country',
          type: 'select',
          appliesTo: [{ field: 'country_id', op: 'eq' }],
        },
        {
          id: 'customer_filter',
          label: 'Customer',
          type: 'select',
          appliesTo: [{ blockId: 'orders', field: 'customer_id', op: 'eq' }],
          options: {
            source: 'object_model',
            schema: 'customers',
            field: 'id',
            valueField: 'id',
            labelField: 'name',
            dependsOn: ['country_filter'],
            filterMappings,
            condition: optionsCondition,
          },
        },
      ],
      blocks: [
        {
          id: 'orders',
          type: 'table',
          title: 'Orders',
          source: { schema: 'orders', mode: 'filter' },
          table: {
            columns: [
              { field: 'id', label: 'ID' },
              { field: 'customer_id', label: 'Customer' },
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
    expect(state.filters[1]).toMatchObject({
      target: 'orders',
      field: 'customer_id',
      optionsSchema: 'customers',
      optionsField: 'id',
      optionsValueField: 'id',
      optionsLabelField: 'name',
      dependsOn: ['country_filter'],
      filterMappings,
      optionsCondition,
    });

    const round = wizardStateToDefinition(
      state,
      {
        ...SCHEMA_FIELDS,
        customers: ['id', 'name', 'country_id'],
      },
      definition
    );
    expect(round.filters[1].options).toMatchObject({
      source: 'object_model',
      schema: 'customers',
      field: 'id',
      valueField: 'id',
      labelField: 'name',
      search: true,
      dependsOn: ['country_filter'],
    });
    expect(round.filters[1].options?.filterMappings).toEqual(filterMappings);
    expect(round.filters[1].options?.condition).toEqual(optionsCondition);
  });

  it('round-trips filters with multiple target mappings', () => {
    const appliesTo = [
      { blockId: 'orders', field: 'status', op: 'eq' },
      { blockId: 'order_summary', field: 'status', op: 'eq' },
    ];
    const definition: ReportDefinition = {
      definitionVersion: 1,
      filters: [
        {
          id: 'status_filter',
          label: 'Status',
          type: 'select',
          appliesTo,
        },
      ],
      blocks: [
        {
          id: 'orders',
          type: 'table',
          title: 'Orders',
          source: { schema: 'orders', mode: 'filter' },
          table: {
            columns: [
              { field: 'id', label: 'ID' },
              { field: 'status', label: 'Status' },
            ],
          },
        },
        {
          id: 'order_summary',
          type: 'table',
          title: 'Order summary',
          source: { schema: 'orders', mode: 'filter' },
          table: {
            columns: [
              { field: 'status', label: 'Status' },
              { field: 'amount', label: 'Amount' },
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
    expect(compatibility.reasons).not.toContain(
      'Filter with multiple target mappings'
    );
    expect(state.filters[0]).toMatchObject({
      target: WIZARD_FILTER_TARGET_CUSTOM,
      targetMappings: appliesTo,
      field: 'status',
    });

    const round = wizardStateToDefinition(state, SCHEMA_FIELDS, definition);
    expect(round.filters[0].appliesTo).toEqual(appliesTo);
  });

  it('round-trips block click interactions', () => {
    const definition: ReportDefinition = {
      definitionVersion: 1,
      filters: [
        {
          id: 'order_id',
          label: 'Order',
          type: 'select',
          appliesTo: [{ field: 'id', op: 'eq' }],
        },
      ],
      blocks: [
        {
          id: 'orders',
          type: 'table',
          title: 'Orders',
          source: { schema: 'orders', mode: 'filter' },
          table: {
            columns: [{ field: 'id', label: 'ID' }],
          },
          interactions: [
            {
              id: 'open_order',
              trigger: { event: 'row_click' },
              actions: [
                {
                  type: 'set_filter',
                  filterId: 'order_id',
                  valueFrom: 'datum.id',
                },
                {
                  type: 'navigate_view',
                  viewId: 'detail',
                },
              ],
            },
          ],
        },
      ],
    };

    const { state, compatibility } = definitionToWizardState(
      definition,
      'orders'
    );
    expect(compatibility.fullyEditable).toBe(true);
    expect(state.blocks[0].interactions).toEqual(
      definition.blocks[0].interactions
    );

    const round = wizardStateToDefinition(state, SCHEMA_FIELDS, definition);
    expect(round.blocks[0].interactions).toEqual(
      definition.blocks[0].interactions
    );
  });

  it('round-trips views and breadcrumb metadata without compatibility warning', () => {
    const definition: ReportDefinition = {
      definitionVersion: 1,
      filters: [],
      layout: [
        {
          id: 'main',
          type: 'grid',
          columns: 1,
          items: [{ blockId: 'orders' }],
        },
      ],
      views: [
        {
          id: 'list',
          title: 'Orders',
          layout: [
            {
              id: 'list_grid',
              type: 'grid',
              columns: 1,
              items: [{ blockId: 'orders' }],
            },
          ],
        },
        {
          id: 'detail',
          titleFrom: 'filters.order_id',
          parentViewId: 'list',
          clearFiltersOnBack: ['order_id'],
          breadcrumb: [
            {
              label: 'Orders',
              viewId: 'list',
              clearFilters: ['order_id'],
            },
          ],
          layout: [
            {
              id: 'detail_grid',
              type: 'grid',
              columns: 1,
              items: [{ blockId: 'orders' }],
            },
          ],
        },
      ],
      blocks: [
        {
          id: 'orders',
          type: 'table',
          title: 'Orders',
          source: { schema: 'orders', mode: 'filter' },
          table: { columns: [{ field: 'id' }] },
        },
      ],
    };

    const { state, compatibility } = definitionToWizardState(
      definition,
      'orders'
    );
    expect(compatibility.fullyEditable).toBe(true);
    expect(state.views).toEqual(definition.views);

    const round = wizardStateToDefinition(state, SCHEMA_FIELDS, definition);
    expect(round.views).toEqual(definition.views);
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
          dimensions: [{ field: 'status', label: 'Status', type: 'string' }],
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
