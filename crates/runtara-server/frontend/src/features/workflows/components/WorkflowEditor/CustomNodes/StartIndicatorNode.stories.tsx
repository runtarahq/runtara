import type { Meta, StoryObj } from '@storybook/react';
import { Node, Edge } from '@xyflow/react';
import { StartIndicatorNode } from './StartIndicatorNode';
import { ReactFlowStoryWrapper, resetStores, setStoreEdges } from './storybook';
import { NODE_TYPES } from '@/features/workflows/config/workflow';

const NODE_ID = 'start-indicator-1';

// Helper to create a StartIndicatorNode
interface CreateStartIndicatorNodeOptions {
  hasEntryPoint?: boolean;
  selected?: boolean;
}

function createStartIndicatorNode(
  opts: CreateStartIndicatorNodeOptions = {}
): Node {
  const { hasEntryPoint = true, selected = false } = opts;

  return {
    id: NODE_ID,
    type: NODE_TYPES.StartIndicatorNode,
    position: { x: 50, y: 50 },
    selected,
    data: {
      hasEntryPoint,
      onAddFirstStep: () => console.log('Add first step clicked'),
    },
  };
}

// Create a target node for edge stories
function createTargetNode(id: string): Node {
  return {
    id,
    type: NODE_TYPES.BasicNode,
    position: { x: 250, y: 50 },
    data: {
      id,
      name: 'First Step',
      stepType: 'Agent',
      agentId: '',
      capabilityId: '',
      inputMapping: [],
    },
  };
}

const meta: Meta<typeof StartIndicatorNode> = {
  title: 'WorkflowEditor/Nodes/StartIndicatorNode',
  component: StartIndicatorNode,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'Visual entry point indicator. Shows where the workflow starts. Not an actual step in execution.',
      },
    },
  },
  tags: ['autodocs'],
};

export default meta;
type Story = StoryObj<typeof StartIndicatorNode>;

// With entry point (connected to first step)
export const WithEntryPoint: Story = {
  name: 'With Entry Point',
  decorators: [
    (Story) => {
      resetStores();
      const edges: Edge[] = [
        {
          id: 'e-start',
          source: NODE_ID,
          sourceHandle: 'onstart',
          target: 'first-step',
          targetHandle: 'target',
        },
      ];
      setStoreEdges(edges);
      return <Story />;
    },
  ],
  render: () => {
    const edges: Edge[] = [
      {
        id: 'e-start',
        source: NODE_ID,
        sourceHandle: 'onstart',
        target: 'first-step',
        targetHandle: 'target',
      },
    ];
    return (
      <ReactFlowStoryWrapper
        nodes={[createStartIndicatorNode(), createTargetNode('first-step')]}
        edges={edges}
        width={500}
        height={200}
      />
    );
  },
};

// Without entry point (shows add button)
export const WithoutEntryPoint: Story = {
  name: 'Without Entry Point (Add Button)',
  decorators: [
    (Story) => {
      resetStores();
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[createStartIndicatorNode({ hasEntryPoint: false })]}
      height={200}
      width={300}
    />
  ),
};

// Standalone (no connection)
export const Standalone: Story = {
  name: 'Standalone',
  decorators: [
    (Story) => {
      resetStores();
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[createStartIndicatorNode()]}
      height={200}
      width={300}
    />
  ),
};

// Comparison view
export const Comparison: Story = {
  name: 'States Comparison',
  parameters: {
    layout: 'padded',
  },
  decorators: [
    (Story) => {
      resetStores();
      return <Story />;
    },
  ],
  render: () => (
    <div className="grid grid-cols-2 gap-4">
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          Has Entry Point
        </p>
        <ReactFlowStoryWrapper
          nodes={[
            {
              id: 'start-with',
              type: NODE_TYPES.StartIndicatorNode,
              position: { x: 50, y: 50 },
              data: { hasEntryPoint: true },
            },
          ]}
          height={150}
          width={250}
        />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          No Entry Point (Shows Add Button)
        </p>
        <ReactFlowStoryWrapper
          nodes={[
            {
              id: 'start-without',
              type: NODE_TYPES.StartIndicatorNode,
              position: { x: 50, y: 50 },
              data: {
                hasEntryPoint: false,
                onAddFirstStep: () => {},
              },
            },
          ]}
          height={150}
          width={250}
        />
      </div>
    </div>
  ),
};
