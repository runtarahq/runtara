import { ReactNode } from 'react';
import { ReactFlow, Node, Edge, NodeTypes } from '@xyflow/react';
import { MemoryRouter } from 'react-router';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import '@xyflow/react/dist/base.css';

// Import all custom node types
import { NODE_TYPES } from '@/features/workflows/config/workflow';
import {
  BasicNode,
  ConditionalNode,
  ContainerNode,
  CreateNode,
  EventNode,
  SwitchNode,
  NoteNode,
  StartIndicatorNode,
} from '../index';

// Register all node types (same as WorkflowEditor/index.tsx)
const nodeTypes: NodeTypes = {
  [NODE_TYPES.BasicNode]: BasicNode,
  [NODE_TYPES.CreateNode]: CreateNode,
  [NODE_TYPES.ConditionalNode]: ConditionalNode,
  [NODE_TYPES.SwitchNode]: SwitchNode,
  [NODE_TYPES.ContainerNode]: ContainerNode,
  [NODE_TYPES.NoteNode]: NoteNode,
  [NODE_TYPES.StartIndicatorNode]: StartIndicatorNode,
  // EventNode not in config but used in WorkflowEditor
  EVENT_NODE: EventNode,
};

// QueryClient for stories - no actual fetching
const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: false,
      staleTime: Infinity,
      refetchOnWindowFocus: false,
    },
  },
});

interface ReactFlowStoryWrapperProps {
  /** Nodes to render in the canvas */
  nodes: Node[];
  /** Edges to render (for showing connections) */
  edges?: Edge[];
  /** Canvas width in pixels */
  width?: number;
  /** Canvas height in pixels */
  height?: number;
  /** Optional children to render inside the wrapper */
  children?: ReactNode;
}

/**
 * Wrapper component for rendering React Flow nodes in Storybook.
 * Provides all necessary context: ReactFlow, Router, QueryClient.
 * Canvas interactions are disabled for static display.
 */
export function ReactFlowStoryWrapper({
  nodes,
  edges = [],
  width = 500,
  height = 300,
  children,
}: ReactFlowStoryWrapperProps) {
  return (
    <MemoryRouter>
      <QueryClientProvider client={queryClient}>
        <div
          style={{ width, height }}
          className="border border-border rounded-lg overflow-hidden"
        >
          <ReactFlow
            nodes={nodes}
            edges={edges}
            nodeTypes={nodeTypes}
            fitView
            fitViewOptions={{ padding: 0.5 }}
            // Disable all interactivity for static display
            nodesDraggable={false}
            nodesConnectable={false}
            elementsSelectable={false}
            panOnDrag={false}
            zoomOnScroll={false}
            zoomOnPinch={false}
            zoomOnDoubleClick={false}
            preventScrolling={false}
            // Clean appearance
            proOptions={{ hideAttribution: true }}
          >
            {children}
          </ReactFlow>
        </div>
      </QueryClientProvider>
    </MemoryRouter>
  );
}
