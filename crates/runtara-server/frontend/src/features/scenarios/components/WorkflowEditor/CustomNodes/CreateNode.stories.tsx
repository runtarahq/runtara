import type { Meta, StoryObj } from '@storybook/react';
import { Node } from '@xyflow/react';
import { CreateNode } from './CreateNode';
import { ReactFlowStoryWrapper, resetStores, setExecuting } from './storybook';
import { NODE_TYPES } from '@/features/scenarios/config/workflow';

const NODE_ID = 'create-1';

// Helper to create a CreateNode
interface CreateCreateNodeOptions {
  selected?: boolean;
}

function createCreateNode(opts: CreateCreateNodeOptions = {}): Node {
  const { selected = false } = opts;

  return {
    id: NODE_ID,
    type: NODE_TYPES.CreateNode,
    position: { x: 100, y: 50 },
    selected,
    data: {
      id: NODE_ID,
      name: '',
      stepType: 'Create',
      agentId: '',
      capabilityId: '',
      inputMapping: [],
    },
  };
}

const meta: Meta<typeof CreateNode> = {
  title: 'WorkflowEditor/Nodes/CreateNode',
  component: CreateNode,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'Temporary node for creating new steps. Click to open the step creation dialog. Hidden during execution.',
      },
    },
  },
  tags: ['autodocs'],
};

export default meta;
type Story = StoryObj<typeof CreateNode>;

// Default state
export const Default: Story = {
  decorators: [
    (Story) => {
      resetStores();
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper nodes={[createCreateNode()]} height={200} />
  ),
};

// Selected state
export const Selected: Story = {
  decorators: [
    (Story) => {
      resetStores();
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[createCreateNode({ selected: true })]}
      height={200}
    />
  ),
};

// During execution (hidden)
export const DuringExecution: Story = {
  name: 'During Execution (Hidden)',
  decorators: [
    (Story) => {
      resetStores();
      setExecuting();
      return <Story />;
    },
  ],
  render: () => (
    <div>
      <p className="text-sm text-muted-foreground mb-4">
        CreateNode returns null during execution and is not visible:
      </p>
      <ReactFlowStoryWrapper nodes={[createCreateNode()]} height={200} />
    </div>
  ),
};

// Comparison
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
          Default
        </p>
        <ReactFlowStoryWrapper
          nodes={[
            {
              id: 'create-default',
              type: NODE_TYPES.CreateNode,
              position: { x: 100, y: 50 },
              data: {
                id: 'create-default',
                name: '',
                stepType: 'Create',
                agentId: '',
                capabilityId: '',
                inputMapping: [],
              },
            },
          ]}
          height={150}
          width={350}
        />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          Selected
        </p>
        <ReactFlowStoryWrapper
          nodes={[
            {
              id: 'create-selected',
              type: NODE_TYPES.CreateNode,
              position: { x: 100, y: 50 },
              selected: true,
              data: {
                id: 'create-selected',
                name: '',
                stepType: 'Create',
                agentId: '',
                capabilityId: '',
                inputMapping: [],
              },
            },
          ]}
          height={150}
          width={350}
        />
      </div>
    </div>
  ),
};
