import { describe, expect, it } from 'vitest';
import {
  cardDetailLayout,
  describeCondition,
  describeReference,
  humanizeKey,
  stepDetailRows,
} from './step-details';
import { stepDescription } from './step-summary';

const stepNames = new Map([['read', 'Read tenant record']]);

describe('humanizeKey', () => {
  it('turns field keys into labels', () => {
    expect(humanizeKey('instance_id')).toBe('Instance ID');
    expect(humanizeKey('schemaName')).toBe('Schema name');
    expect(humanizeKey('last_transition')).toBe('Last transition');
    expect(humanizeKey('callback_url')).toBe('Callback URL');
    expect(humanizeKey('update-instance')).toBe('Update instance');
  });
});

describe('describeReference', () => {
  it('names where a value comes from', () => {
    expect(describeReference('data.id')).toEqual({
      source: 'Input',
      path: 'id',
    });
    expect(describeReference('workflow.inputs.data.customer.email')).toEqual({
      source: 'Input',
      path: 'customer.email',
    });
    expect(describeReference('data.sku', { dataScope: 'item' })).toEqual({
      source: 'Item',
      path: 'sku',
    });
    expect(describeReference('steps.read.outputs.rows', { stepNames })).toEqual(
      { source: 'Read tenant record', path: 'rows' }
    );
    expect(describeReference('steps.unknown.outputs')).toEqual({
      source: 'unknown',
      path: '',
    });
    expect(describeReference('steps.__error.message')).toEqual({
      source: 'Error',
      path: 'message',
    });
    expect(describeReference('variables.region')).toEqual({
      source: 'Variable',
      path: 'region',
    });
    expect(describeReference('variables._instance_id')).toEqual({
      source: 'Run',
      path: 'Instance ID',
    });
  });
});

describe('stepDetailRows', () => {
  // The step from the Object Model update-instance screenshot.
  const updateInstance = {
    stepType: 'Agent',
    inputMapping: [
      {
        type: 'data',
        valueType: 'composite',
        value: {
          stage: { valueType: 'immediate', value: 'published' },
          last_transition: {
            valueType: 'reference',
            value: 'steps.read.outputs.at',
          },
          updated_by: { valueType: 'reference', value: 'data.user' },
        },
      },
      { type: 'instance_id', value: 'data.id', valueType: 'reference' },
      {
        type: 'schema_name',
        value: 'ReportStageCase',
        valueType: 'immediate',
      },
    ],
  };

  it('reads as labelled inputs with named sources', () => {
    const rows = stepDetailRows(updateInstance, { stepNames });
    expect(rows.map((row) => [row.label, row.value])).toEqual([
      ['Data', 'Stage, Last transition, Updated by'],
      ['Instance ID', 'Input · id'],
      ['Schema name', 'ReportStageCase'],
    ]);
    expect(rows[0].children?.map((row) => [row.label, row.value])).toEqual([
      ['Stage', 'published'],
      ['Last transition', 'Read tenant record · at'],
      ['Updated by', 'Input · user'],
    ]);
  });

  it("uses the capability's labels and puts required inputs first", () => {
    const rows = stepDetailRows(updateInstance, {
      inputFields: [
        { name: 'schema_name', displayName: 'Schema', required: true },
        { name: 'instance_id', displayName: 'Instance', required: true },
        { name: 'data', displayName: 'Fields to update', required: false },
      ],
    });
    expect(rows.map((row) => row.label)).toEqual([
      'Schema',
      'Instance',
      'Fields to update',
    ]);
  });

  it('never changes the row count with context', () => {
    expect(stepDetailRows(updateInstance)).toHaveLength(
      stepDetailRows(updateInstance, { stepNames, dataScope: 'item' }).length
    );
  });

  it('renders template references as sources', () => {
    const [row] = stepDetailRows(
      {
        stepType: 'Log',
        inputMapping: [
          {
            type: 'message',
            value: 'Order {{ data.orderId }} for {{ steps.read.outputs.name }}',
            valueType: 'template',
          },
        ],
      },
      { stepNames }
    );
    expect(row.value).toBe(
      'Order ‹Input · orderId› for ‹Read tenant record · name›'
    );
  });

  it('skips empty inputs and editor plumbing', () => {
    const rows = stepDetailRows({
      stepType: 'Switch',
      inputMapping: [
        { type: 'value', value: 'data.tier', valueType: 'reference' },
        { type: 'cases', value: [{ match: 'gold' }] },
        { type: 'routingMode', value: false },
        { type: 'default', value: '', valueType: 'immediate' },
      ],
    });
    expect(rows.map((row) => row.key)).toEqual(['value']);
  });

  it('formats durations, flags and iterated collections', () => {
    expect(
      stepDetailRows({
        stepType: 'Delay',
        inputMapping: [
          { type: 'durationMs', value: 1200, valueType: 'immediate' },
          { type: 'retry', value: true, valueType: 'immediate' },
        ],
      }).map((row) => [row.label, row.value])
    ).toEqual([
      ['Duration', '1.2 s'],
      ['Retry', 'Yes'],
    ]);
    expect(
      stepDetailRows({
        stepType: 'Split',
        splitParallelism: 4,
        inputMapping: [
          { type: 'value', value: 'data.orders', valueType: 'reference' },
        ],
      }).map((row) => [row.label, row.value])
    ).toEqual([
      ['Items', 'Input · orders'],
      ['Parallelism', '4 at a time'],
    ]);
  });

  it('says what a signal wait asks for', () => {
    const rows = stepDetailRows({
      stepType: 'WaitForSignal',
      inputMapping: [
        {
          type: 'responseSchema',
          value: [{ name: 'decision' }, { name: 'review_note' }],
        },
        { type: 'timeoutMs', value: '', valueType: 'immediate' },
      ],
    });
    expect(rows.map((row) => [row.label, row.value])).toEqual([
      ['Asks for', 'Decision, Review note'],
    ]);
  });

  it('leaves Conditional and AiAgent to their own cards', () => {
    expect(
      stepDetailRows({
        stepType: 'Conditional',
        inputMapping: [{ type: 'other', value: 'x' }],
      })
    ).toEqual([]);
    expect(
      stepDetailRows({
        stepType: 'AiAgent',
        inputMapping: [{ type: 'model', value: 'gpt' }],
      })
    ).toEqual([]);
  });
});

describe('stepDescription', () => {
  it('names the agent and capability', () => {
    const step = {
      stepType: 'Agent',
      agentId: 'object-model',
      capabilityId: 'update-instance',
    };
    expect(stepDescription(step, 'Object Model', 'Update Instance')).toBe(
      'Object Model · Update Instance'
    );
    expect(stepDescription(step)).toBe('Object model · Update instance');
  });
});

describe('cardDetailLayout', () => {
  const rows = (count: number) =>
    Array.from({ length: count }, (_, index) => ({
      key: `k${index}`,
      label: `k${index}`,
      value: 'v',
      kind: 'literal' as const,
    }));

  it('shows every row while they fit', () => {
    expect(cardDetailLayout(rows(3))).toMatchObject({
      hiddenCount: 0,
      lineCount: 3,
    });
  });

  it('collapses the overflow into a "+N more" line', () => {
    const layout = cardDetailLayout(rows(6));
    expect(layout.visible).toHaveLength(2);
    expect(layout.hiddenCount).toBe(4);
    expect(layout.lineCount).toBe(3);
  });
});

describe('describeCondition', () => {
  it('reads a condition as a sentence', () => {
    expect(
      describeCondition({
        type: 'operation',
        op: 'EQ',
        arguments: [
          { value: 'data.stage', valueType: 'reference' },
          { value: 'intake', valueType: 'immediate' },
        ],
      })
    ).toBe('Input · stage is "intake"');
    expect(
      describeCondition(
        {
          op: 'AND',
          arguments: [
            {
              op: 'GTE',
              arguments: [
                { value: 'steps.read.outputs.total', valueType: 'reference' },
                { value: 100, valueType: 'immediate' },
              ],
            },
            {
              op: 'IS_NOT_EMPTY',
              arguments: [{ value: 'data.email', valueType: 'reference' }],
            },
          ],
        },
        { stepNames }
      )
    ).toBe(
      '(Read tenant record · total ≥ 100) and (Input · email is not empty)'
    );
  });
});
