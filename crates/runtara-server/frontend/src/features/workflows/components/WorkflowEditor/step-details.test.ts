import { describe, expect, it } from 'vitest';
import { cardDetailLayout, stepDetailRows } from './step-details';

describe('stepDetailRows', () => {
  it('lists configured inputs with their value kind', () => {
    const rows = stepDetailRows({
      stepType: 'Finish',
      inputMapping: [
        {
          type: 'decision',
          value: 'steps.request_decision.outputs',
          valueType: 'reference',
        },
        { type: 'status', value: 'Completed', valueType: 'immediate' },
        {
          type: 'summary',
          value: 'Order {{data.orderId}} done',
          valueType: 'template',
        },
        {
          type: 'payload',
          value: { id: {}, total: {} },
          valueType: 'composite',
        },
      ],
    });
    expect(rows).toEqual([
      {
        key: 'decision',
        label: 'decision',
        value: 'steps.request_decision.outputs',
        kind: 'reference',
      },
      {
        key: 'status',
        label: 'status',
        value: 'Completed',
        kind: 'literal',
      },
      {
        key: 'summary',
        label: 'summary',
        value: 'Order {{data.orderId}} done',
        kind: 'template',
      },
      {
        key: 'payload',
        label: 'payload',
        value: '{ id, total }',
        kind: 'composite',
      },
    ]);
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

  it('formats durations and names iterated collections', () => {
    expect(
      stepDetailRows({
        stepType: 'Delay',
        inputMapping: [
          { type: 'durationMs', value: 1200, valueType: 'immediate' },
        ],
      })
    ).toEqual([
      { key: 'durationMs', label: 'duration', value: '1.2 s', kind: 'literal' },
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
      ['items', 'data.orders'],
      ['parallelism', '4'],
    ]);
  });

  it('says what a signal wait asks for', () => {
    const rows = stepDetailRows({
      stepType: 'WaitForSignal',
      inputMapping: [
        {
          type: 'responseSchema',
          value: [{ name: 'decision' }, { name: 'note' }],
        },
        { type: 'timeoutMs', value: '', valueType: 'immediate' },
      ],
    });
    expect(rows).toEqual([
      {
        key: 'responseSchema',
        label: 'asks for',
        value: 'decision, note',
        kind: 'list',
      },
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
