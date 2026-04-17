import type { Meta, StoryObj } from '@storybook/react';
import { Node } from '@xyflow/react';
import { ContainerNode } from './ContainerNode';
import {
  ReactFlowStoryWrapper,
  resetStores,
  setNodeValidationError,
  setNodeExecutionStatus,
  setExecuting,
} from './storybook';
import { ExecutionStatus } from '@/generated/RuntaraRuntimeApi';
import { NODE_TYPES } from '@/features/scenarios/config/workflow';

const NODE_ID = 'container-1';

// Helper to create a ContainerNode
interface CreateContainerNodeOptions {
  name?: string;
  selected?: boolean;
  width?: number;
  height?: number;
}

function createContainerNode(opts: CreateContainerNodeOptions = {}): Node {
  const {
    name = 'Parallel Processing',
    selected = false,
    width = 204,
    height = 168,
  } = opts;

  return {
    id: NODE_ID,
    type: NODE_TYPES.ContainerNode,
    position: { x: 50, y: 30 },
    selected,
    style: { width, height },
    data: {
      id: NODE_ID,
      name,
      stepType: 'Split',
      agentId: '',
      capabilityId: '',
      inputMapping: [],
    },
  };
}

const meta: Meta<typeof ContainerNode> = {
  title: 'WorkflowEditor/Nodes/ContainerNode',
  component: ContainerNode,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'Container node for parallel/split execution. Can hold child nodes inside. Resizable.',
      },
    },
  },
  tags: ['autodocs'],
};

export default meta;
type Story = StoryObj<typeof ContainerNode>;

// Default empty container
export const Default: Story = {
  name: 'Default (Empty)',
  decorators: [
    (Story) => {
      resetStores();
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[createContainerNode()]}
      height={300}
      width={400}
    />
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
      nodes={[createContainerNode({ selected: true })]}
      height={300}
      width={400}
    />
  ),
};

// Larger container
export const LargeContainer: Story = {
  name: 'Large Container',
  decorators: [
    (Story) => {
      resetStores();
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[
        createContainerNode({
          name: 'Large Split',
          width: 350,
          height: 250,
        }),
      ]}
      height={350}
      width={500}
    />
  ),
};

// Execution Running
export const ExecutionRunning: Story = {
  name: 'Execution: Running',
  decorators: [
    (Story) => {
      resetStores();
      setNodeExecutionStatus(NODE_ID, ExecutionStatus.Running);
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[createContainerNode()]}
      height={300}
      width={400}
    />
  ),
};

// Execution Completed
export const ExecutionCompleted: Story = {
  name: 'Execution: Completed',
  decorators: [
    (Story) => {
      resetStores();
      setNodeExecutionStatus(NODE_ID, ExecutionStatus.Completed, {
        executionTime: 3450,
      });
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[createContainerNode()]}
      height={300}
      width={400}
    />
  ),
};

// Validation Error
export const ValidationError: Story = {
  name: 'Validation Error',
  decorators: [
    (Story) => {
      resetStores();
      setNodeValidationError(NODE_ID);
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[createContainerNode()]}
      height={300}
      width={400}
    />
  ),
};

// During Execution
export const DuringExecution: Story = {
  name: 'During Execution (Read-Only)',
  decorators: [
    (Story) => {
      resetStores();
      setExecuting();
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[createContainerNode()]}
      height={300}
      width={400}
    />
  ),
};

// All states
export const AllStates: Story = {
  name: 'All States Reference',
  parameters: {
    layout: 'padded',
  },
  decorators: [
    (Story) => {
      resetStores();
      setNodeValidationError('container-error');
      setNodeExecutionStatus('container-running', ExecutionStatus.Running);
      setNodeExecutionStatus('container-completed', ExecutionStatus.Completed, {
        executionTime: 3450,
      });
      return <Story />;
    },
  ],
  render: () => {
    const createStateNode = (id: string, name: string): Node => ({
      id,
      type: NODE_TYPES.ContainerNode,
      position: { x: 50, y: 30 },
      style: { width: 204, height: 168 },
      data: {
        id,
        name,
        stepType: 'Split',
        agentId: '',
        capabilityId: '',
        inputMapping: [],
      },
    });

    return (
      <div className="grid grid-cols-2 gap-4">
        <div>
          <p className="text-xs font-medium mb-2 text-muted-foreground">
            Default
          </p>
          <ReactFlowStoryWrapper
            nodes={[createStateNode('container-default', 'Default')]}
            height={250}
            width={350}
          />
        </div>
        <div>
          <p className="text-xs font-medium mb-2 text-muted-foreground">
            Validation Error
          </p>
          <ReactFlowStoryWrapper
            nodes={[createStateNode('container-error', 'Error')]}
            height={250}
            width={350}
          />
        </div>
        <div>
          <p className="text-xs font-medium mb-2 text-muted-foreground">
            Execution: Running
          </p>
          <ReactFlowStoryWrapper
            nodes={[createStateNode('container-running', 'Running')]}
            height={250}
            width={350}
          />
        </div>
        <div>
          <p className="text-xs font-medium mb-2 text-muted-foreground">
            Execution: Completed
          </p>
          <ReactFlowStoryWrapper
            nodes={[createStateNode('container-completed', 'Completed')]}
            height={250}
            width={350}
          />
        </div>
      </div>
    );
  },
};
