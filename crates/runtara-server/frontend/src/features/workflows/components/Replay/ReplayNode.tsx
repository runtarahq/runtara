/**
 * Read-only React Flow node for the replay graph. Reuses {@link BaseNode} for
 * icon + name + the replay ring/glow/badge, and exposes a single left/right
 * handle pair so the auto-laid-out edges attach cleanly.
 */
import { memo } from 'react';
import { Handle, Position, type Node, type NodeProps } from '@xyflow/react';
import { BaseNode } from '../WorkflowEditor/BaseNode';
import type { ReplayIterationCounts, ReplayNodeState } from './types';

export interface ReplayNodeData extends Record<string, unknown> {
  stepType: string;
  name: string;
  replayState: ReplayNodeState;
  replayIteration?: ReplayIterationCounts;
}

export type ReplayFlowNode = Node<ReplayNodeData, 'replayStep'>;

function ReplayNodeComponent({ data, selected }: NodeProps<ReplayFlowNode>) {
  return (
    <BaseNode
      name={data.name}
      stepType={data.stepType}
      selected={selected}
      replayState={data.replayState}
      replayIteration={data.replayIteration}
      className="cursor-pointer"
    >
      <Handle
        type="target"
        id="target"
        position={Position.Left}
        isConnectable={false}
        className="!bg-muted-foreground/40 !w-1.5 !h-1.5 !border-0"
      />
      <Handle
        type="source"
        id="source"
        position={Position.Right}
        isConnectable={false}
        className="!bg-muted-foreground/40 !w-1.5 !h-1.5 !border-0"
      />
    </BaseNode>
  );
}

export const ReplayNode = memo(ReplayNodeComponent);
