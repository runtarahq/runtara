/**
 * Read-only replay graph: the versioned DAG auto-laid-out (dagre), with each
 * node's `replayState` and each edge's flow driven by the derived frame. A pure
 * second renderer of the replay model — no editing, no store coupling.
 */
import { useEffect, useMemo, useRef } from 'react';
import '@xyflow/react/dist/base.css';
import {
  Background,
  BackgroundVariant,
  Controls,
  type Edge,
  MiniMap,
  type NodeTypes,
  ReactFlow,
  ReactFlowProvider,
  useReactFlow,
} from '@xyflow/react';
import { layoutDag } from './layoutDag';
import { ReplayNode, type ReplayFlowNode } from './ReplayNode';
import type { ReplayFrame, ReplayModel } from './types';

const NODE_WIDTH = 176;
const NODE_HEIGHT = 46;

const nodeTypes: NodeTypes = { replayStep: ReplayNode };

interface ReplayGraphProps {
  model: ReplayModel;
  frame: ReplayFrame;
  selectedNodeId: string | null;
  onSelectNode: (nodeId: string | null) => void;
}

function isDark(): boolean {
  return (
    typeof document !== 'undefined' &&
    document.documentElement.classList.contains('dark')
  );
}

function ReplayGraphInner({
  model,
  frame,
  selectedNodeId,
  onSelectNode,
}: ReplayGraphProps) {
  const { fitView } = useReactFlow();

  // Layout is stable per graph — recompute only when the node/edge set changes.
  const layoutKey = useMemo(
    () =>
      `${model.nodeIds.join(',')}|${model.edges.map((e) => e.id).join(',')}`,
    [model.nodeIds, model.edges]
  );

  const layout = useMemo(
    () =>
      layoutDag(
        model.nodeIds.map((id) => ({
          id,
          width: NODE_WIDTH,
          height: NODE_HEIGHT,
        })),
        model.edges,
        { direction: 'LR' }
      ),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [layoutKey]
  );

  const baseNodes = useMemo<ReplayFlowNode[]>(
    () =>
      model.nodeIds.map((id) => {
        const node = model.nodes.get(id)!;
        const pos = layout.positions.get(id) ?? { x: 0, y: 0 };
        return {
          id,
          type: 'replayStep',
          position: pos,
          width: NODE_WIDTH,
          height: NODE_HEIGHT,
          style: { width: NODE_WIDTH, height: NODE_HEIGHT },
          selectable: true,
          draggable: false,
          connectable: false,
          data: {
            stepType: node.stepType,
            name: node.name,
            replayState: 'idle',
          },
        } satisfies ReplayFlowNode;
      }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [layoutKey, layout]
  );

  // Apply the current frame's per-node state each tick, but keep referential
  // identity for nodes whose (state, iteration, selection) is unchanged — React
  // Flow + the memoized node then skip re-rendering them. This keeps large
  // graphs smooth under a per-frame clock.
  const prevRef = useRef<{
    sig: Map<string, string>;
    node: Map<string, ReplayFlowNode>;
  }>({ sig: new Map(), node: new Map() });

  const nodes = useMemo<ReplayFlowNode[]>(() => {
    const sigMap = new Map<string, string>();
    const nodeMap = new Map<string, ReplayFlowNode>();
    const out: ReplayFlowNode[] = [];
    for (const base of baseNodes) {
      const state = frame.nodeStates.get(base.id) ?? 'idle';
      const iter = frame.nodeIterations.get(base.id);
      const selected = base.id === selectedNodeId;
      const sig = `${state}|${iter?.active ?? ''}|${iter?.completed ?? ''}|${iter?.total ?? ''}|${selected}`;
      let node =
        prevRef.current.sig.get(base.id) === sig
          ? prevRef.current.node.get(base.id)
          : undefined;
      if (!node) {
        node = {
          ...base,
          selected,
          data: { ...base.data, replayState: state, replayIteration: iter },
        };
      }
      sigMap.set(base.id, sig);
      nodeMap.set(base.id, node);
      out.push(node);
    }
    prevRef.current = { sig: sigMap, node: nodeMap };
    return out;
  }, [baseNodes, frame, selectedNodeId]);

  const edges = useMemo<Edge[]>(
    () =>
      model.edges.map((e) => {
        const active = frame.activeEdges.has(e.id);
        const targetState = frame.nodeStates.get(e.target);
        const reached =
          targetState != null &&
          targetState !== 'idle' &&
          targetState !== 'skipped';
        return {
          id: e.id,
          source: e.source,
          target: e.target,
          sourceHandle: 'source',
          targetHandle: 'target',
          type: 'default',
          animated: active,
          style: {
            stroke: active
              ? 'hsl(var(--primary))'
              : reached
              ? 'hsl(var(--muted-foreground))'
              : 'hsl(var(--border))',
            strokeWidth: active ? 2.5 : 1.5,
            opacity: reached || active ? 1 : 0.5,
            transition: 'stroke 200ms, stroke-width 200ms, opacity 200ms',
          },
        } satisfies Edge;
      }),
    [model.edges, frame]
  );

  // Fit the view once per layout (not per animation frame).
  useEffect(() => {
    const t = window.setTimeout(() => fitView({ padding: 0.18, duration: 200 }), 60);
    return () => window.clearTimeout(t);
  }, [layoutKey, fitView]);

  return (
    <ReactFlow
      nodes={nodes}
      edges={edges}
      nodeTypes={nodeTypes}
      proOptions={{ hideAttribution: true }}
      nodesDraggable={false}
      nodesConnectable={false}
      elementsSelectable
      edgesFocusable={false}
      nodesFocusable={false}
      onNodeClick={(_e, node) => onSelectNode(node.id)}
      onPaneClick={() => onSelectNode(null)}
      fitView
      minZoom={0.2}
      maxZoom={1.75}
    >
      <Background
        variant={BackgroundVariant.Dots}
        gap={20}
        size={1}
        color={isDark() ? '#262626' : undefined}
      />
      <Controls showInteractive={false} />
      <MiniMap
        pannable
        zoomable
        className="!bg-muted/40"
        nodeColor={(n) => {
          const state = (n.data as { replayState?: string })?.replayState;
          switch (state) {
            case 'running':
              return '#3b82f6';
            case 'done':
              return '#22c55e';
            case 'failed':
              return '#ef4444';
            case 'suspended':
              return '#f59e0b';
            default:
              return '#94a3b8';
          }
        }}
      />
    </ReactFlow>
  );
}

export function ReplayGraph(props: ReplayGraphProps) {
  return (
    <ReactFlowProvider>
      <ReplayGraphInner {...props} />
    </ReactFlowProvider>
  );
}
