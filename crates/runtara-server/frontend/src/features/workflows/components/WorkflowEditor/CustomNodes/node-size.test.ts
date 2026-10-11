import { describe, expect, it } from 'vitest';
import type { Node } from '@xyflow/react';
import { NODE_TYPES } from '@/features/workflows/config/workflow.ts';
import { SNAP_GRID_SIZE } from '@/features/workflows/config/workflow-editor';
import { executionGraphToReactFlow, getLayoutedElements } from './utils.tsx';
import {
  aiAgentNodeHeight,
  COMPACT_CARD_HEIGHT,
  DETAILED_CARD_WIDTH,
  detailBlockHeight,
  getStepNodeSize,
  switchRoutingHeight,
} from './node-size';

const isOddGridMultiple = (value: number) =>
  value % SNAP_GRID_SIZE === 0 && (value / SNAP_GRID_SIZE) % 2 === 1;

function boxOf(node: Node) {
  return {
    left: node.position.x,
    top: node.position.y,
    width: node.style?.width as number,
    height: node.style?.height as number,
  };
}

/**
 * The "Landing Demo" workflow as the API returns it: a straight chain whose
 * saved visual state still carries the 132x36 card size from before the card
 * redesign. The canvas packed cards into those boxes while drawing them at
 * full size, so they overlapped and every edge ran underneath a card.
 */
const landingDemoGraph = {
  name: 'Landing Demo',
  entryPoint: 'receive_order',
  steps: {
    receive_order: {
      id: 'receive_order',
      stepType: 'Log',
      name: 'Receive order',
      message: 'Order received from the commerce system',
      level: 'info',
    },
    sync_systems: {
      id: 'sync_systems',
      stepType: 'Delay',
      name: 'Sync systems',
      durationMs: { value: 1200, valueType: 'immediate' },
    },
    request_decision: {
      id: 'request_decision',
      stepType: 'WaitForSignal',
      name: 'Request human decision',
      responseSchema: {
        decision: { type: 'string', required: true },
        note: { type: 'string', required: false },
      },
    },
    finish: {
      id: 'finish',
      stepType: 'Finish',
      name: 'Complete and audit',
      inputMapping: {
        decision: {
          value: 'steps.request_decision.outputs',
          valueType: 'reference',
        },
        status: { value: 'Completed', valueType: 'immediate' },
      },
    },
  },
  executionPlan: [
    { fromStep: 'receive_order', toStep: 'sync_systems', label: 'next' },
    { fromStep: 'sync_systems', toStep: 'request_decision', label: 'next' },
    { fromStep: 'request_decision', toStep: 'finish', label: 'next' },
  ],
  nodes: ['receive_order', 'sync_systems', 'request_decision', 'finish'].map(
    (id, index) => ({
      id,
      type: NODE_TYPES.BasicNode,
      position: { x: 72 + index * 204, y: 240 },
      width: 132,
      height: 36,
    })
  ),
};

describe('canvas node sizes', () => {
  it('lays out cards at the size they render, ignoring stale saved sizes', () => {
    const { nodes, edges } = executionGraphToReactFlow(landingDemoGraph as any);
    for (const detailLevel of ['compact', 'detailed'] as const) {
      const { nodes: laidOut } = getLayoutedElements(nodes, edges, {
        detailLevel,
      });
      const chain = [
        'receive_order',
        'sync_systems',
        'request_decision',
        'finish',
      ].map((id) => laidOut.find((node) => node.id === id)!);

      for (const node of chain) {
        expect(boxOf(node)).toMatchObject(
          getStepNodeSize(node.type, node.data as any, detailLevel)!
        );
      }
      // Consecutive cards keep a real gap for the edge between them.
      for (let index = 1; index < chain.length; index++) {
        const previous = boxOf(chain[index - 1]);
        const current = boxOf(chain[index]);
        expect(current.left - (previous.left + previous.width)).toBeGreaterThan(
          48
        );
      }
      // And their centers line up, so the chain's edges are straight.
      const centers = chain.map((node) => {
        const box = boxOf(node);
        return box.top + box.height / 2;
      });
      expect(new Set(centers).size).toBe(1);
    }
  });

  it('keeps a resizable container at its saved size', () => {
    const { nodes } = executionGraphToReactFlow({
      name: 'container',
      entryPoint: 'loop',
      steps: {
        loop: {
          id: 'loop',
          stepType: 'Split',
          name: 'Loop',
          subgraph: { steps: {}, executionPlan: [], entryPoint: '' },
        },
      },
      executionPlan: [],
      nodes: [
        {
          id: 'loop',
          type: NODE_TYPES.ContainerNode,
          position: { x: 0, y: 0 },
          width: 420,
          height: 228,
        },
      ],
    } as any);
    const loop = nodes.find((node) => node.id === 'loop')!;
    expect(loop.style?.width).toBe(420);
    expect(loop.style?.height).toBe(228);
  });

  it('only produces heights whose centers share the grid phase', () => {
    expect(isOddGridMultiple(COMPACT_CARD_HEIGHT)).toBe(true);
    for (let lines = 1; lines <= 3; lines++) {
      expect(
        isOddGridMultiple(COMPACT_CARD_HEIGHT + detailBlockHeight(lines))
      ).toBe(true);
    }
    for (let cases = 0; cases <= 6; cases++) {
      expect(isOddGridMultiple(switchRoutingHeight(cases))).toBe(true);
    }
    for (let tools = 0; tools <= 6; tools++) {
      expect(isOddGridMultiple(aiAgentNodeHeight(tools))).toBe(true);
    }
  });

  it('grows the AI Agent card with its tools', () => {
    const withTools = (tools: string[]) =>
      getStepNodeSize(NODE_TYPES.AiAgentNode, {
        stepType: 'AiAgent',
        inputMapping: [{ type: 'tools', value: tools }],
      })!.height;
    expect(withTools(['a', 'b', 'c'])).toBeGreaterThan(withTools([]));
  });

  it('makes detailed cards wider and as tall as their detail lines', () => {
    const log = {
      stepType: 'Log',
      inputMapping: [
        { type: 'message', value: 'hello', valueType: 'immediate' },
        { type: 'level', value: 'info', valueType: 'immediate' },
      ],
    };
    expect(getStepNodeSize(NODE_TYPES.BasicNode, log, 'compact')).toEqual({
      width: 216,
      height: COMPACT_CARD_HEIGHT,
    });
    expect(getStepNodeSize(NODE_TYPES.BasicNode, log, 'detailed')).toEqual({
      width: DETAILED_CARD_WIDTH,
      height: COMPACT_CARD_HEIGHT + detailBlockHeight(2),
    });
  });

  it('leaves resizable nodes to their own size', () => {
    expect(getStepNodeSize(NODE_TYPES.ContainerNode, {})).toBeNull();
    expect(getStepNodeSize(NODE_TYPES.NoteNode, {})).toBeNull();
  });
});
