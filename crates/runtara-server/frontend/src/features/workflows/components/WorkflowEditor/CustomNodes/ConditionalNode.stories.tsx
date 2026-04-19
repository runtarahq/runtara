import type { Meta, StoryObj } from '@storybook/react';
import { Node, Edge } from '@xyflow/react';
import { ConditionalNode } from './ConditionalNode';
import {
  ReactFlowStoryWrapper,
  resetStores,
  setStoreEdges,
  setNodeUnsaved,
  setNodeValidationError,
  setNodeExecutionStatus,
  setExecuting,
} from './storybook';
import { ExecutionStatus } from '@/generated/RuntaraRuntimeApi';
import { NODE_TYPES } from '@/features/workflows/config/workflow';

const NODE_ID = 'conditional-1';

// Helper to create a ConditionalNode with specific configuration
interface CreateConditionalNodeOptions {
  name?: string;
  conditionSummary?: string;
  capabilityId?: string;
  selected?: boolean;
}

function createConditionalNode(opts: CreateConditionalNodeOptions = {}): Node {
  const {
    name = 'Check Order Status',
    conditionSummary,
    capabilityId = '',
    selected = false,
  } = opts;

  // Build inputMapping that will produce a condition summary
  const inputMapping: any[] = [];
  if (conditionSummary) {
    inputMapping.push({
      type: 'condition',
      value: conditionSummary,
    });
  }

  return {
    id: NODE_ID,
    type: NODE_TYPES.ConditionalNode,
    position: { x: 100, y: 50 },
    selected,
    data: {
      id: NODE_ID,
      name,
      stepType: 'Conditional',
      agentId: '',
      capabilityId,
      inputMapping,
    },
  };
}

// Create target nodes for edge stories
function createTargetNode(id: string, y: number, name: string): Node {
  return {
    id,
    type: NODE_TYPES.BasicNode,
    position: { x: 400, y },
    data: {
      id,
      name,
      stepType: 'Agent',
      agentId: '',
      capabilityId: '',
      inputMapping: [],
    },
  };
}

const meta: Meta<typeof ConditionalNode> = {
  title: 'WorkflowEditor/Nodes/ConditionalNode',
  component: ConditionalNode,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'Conditional node for two-way branching (if/else). Has target handle and two source handles: true (green) and false (red).',
      },
    },
  },
  tags: ['autodocs'],
};

export default meta;
type Story = StoryObj<typeof ConditionalNode>;

// Default state
export const Default: Story = {
  decorators: [
    (Story) => {
      resetStores();
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper nodes={[createConditionalNode()]} height={200} />
  ),
};

// With condition summary
export const WithConditionSummary: Story = {
  name: 'With Condition Summary',
  decorators: [
    (Story) => {
      resetStores();
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[
        createConditionalNode({
          name: 'Is Premium Customer',
          conditionSummary: 'customer.tier == "premium"',
        }),
      ]}
      height={200}
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
      nodes={[createConditionalNode({ selected: true })]}
      height={200}
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
    <ReactFlowStoryWrapper nodes={[createConditionalNode()]} height={200} />
  ),
};

// Execution Completed
export const ExecutionCompleted: Story = {
  name: 'Execution: Completed',
  decorators: [
    (Story) => {
      resetStores();
      setNodeExecutionStatus(NODE_ID, ExecutionStatus.Completed, {
        executionTime: 15,
      });
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper nodes={[createConditionalNode()]} height={200} />
  ),
};

// Execution Failed
export const ExecutionFailed: Story = {
  name: 'Execution: Failed',
  decorators: [
    (Story) => {
      resetStores();
      setNodeExecutionStatus(NODE_ID, ExecutionStatus.Failed, {
        error: 'Condition evaluation failed: undefined variable',
      });
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper nodes={[createConditionalNode()]} height={200} />
  ),
};

// Unsaved changes
export const UnsavedChanges: Story = {
  name: 'Unsaved Changes',
  decorators: [
    (Story) => {
      resetStores();
      setNodeUnsaved(NODE_ID);
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper nodes={[createConditionalNode()]} height={200} />
  ),
};

// Validation error
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
    <ReactFlowStoryWrapper nodes={[createConditionalNode()]} height={200} />
  ),
};

// With both branches connected
export const WithConnectedBranches: Story = {
  name: 'With Connected Branches',
  decorators: [
    (Story) => {
      resetStores();
      const edges: Edge[] = [
        {
          id: 'e-true',
          source: NODE_ID,
          sourceHandle: 'true',
          target: 'target-true',
          targetHandle: 'target',
        },
        {
          id: 'e-false',
          source: NODE_ID,
          sourceHandle: 'false',
          target: 'target-false',
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
        id: 'e-true',
        source: NODE_ID,
        sourceHandle: 'true',
        target: 'target-true',
        targetHandle: 'target',
      },
      {
        id: 'e-false',
        source: NODE_ID,
        sourceHandle: 'false',
        target: 'target-false',
        targetHandle: 'target',
      },
    ];
    return (
      <ReactFlowStoryWrapper
        nodes={[
          createConditionalNode(),
          createTargetNode('target-true', 20, 'True Branch'),
          createTargetNode('target-false', 100, 'False Branch'),
        ]}
        edges={edges}
        width={600}
        height={250}
      />
    );
  },
};

// During execution
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
    <ReactFlowStoryWrapper nodes={[createConditionalNode()]} height={200} />
  ),
};

// All states reference
export const AllStates: Story = {
  name: 'All States Reference',
  parameters: {
    layout: 'padded',
  },
  decorators: [
    (Story) => {
      resetStores();
      setNodeUnsaved('cond-unsaved');
      setNodeValidationError('cond-error');
      setNodeExecutionStatus('cond-running', ExecutionStatus.Running);
      setNodeExecutionStatus('cond-completed', ExecutionStatus.Completed, {
        executionTime: 15,
      });
      setNodeExecutionStatus('cond-failed', ExecutionStatus.Failed, {
        error: 'Error',
      });
      return <Story />;
    },
  ],
  render: () => {
    const createStateNode = (id: string, name: string): Node => ({
      id,
      type: NODE_TYPES.ConditionalNode,
      position: { x: 100, y: 50 },
      data: {
        id,
        name,
        stepType: 'Conditional',
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
            nodes={[createStateNode('cond-default', 'Default')]}
            height={150}
            width={350}
          />
        </div>
        <div>
          <p className="text-xs font-medium mb-2 text-muted-foreground">
            Unsaved Changes
          </p>
          <ReactFlowStoryWrapper
            nodes={[createStateNode('cond-unsaved', 'Unsaved')]}
            height={150}
            width={350}
          />
        </div>
        <div>
          <p className="text-xs font-medium mb-2 text-muted-foreground">
            Validation Error
          </p>
          <ReactFlowStoryWrapper
            nodes={[createStateNode('cond-error', 'Error')]}
            height={150}
            width={350}
          />
        </div>
        <div>
          <p className="text-xs font-medium mb-2 text-muted-foreground">
            Execution: Running
          </p>
          <ReactFlowStoryWrapper
            nodes={[createStateNode('cond-running', 'Running')]}
            height={150}
            width={350}
          />
        </div>
        <div>
          <p className="text-xs font-medium mb-2 text-muted-foreground">
            Execution: Completed
          </p>
          <ReactFlowStoryWrapper
            nodes={[createStateNode('cond-completed', 'Completed')]}
            height={150}
            width={350}
          />
        </div>
        <div>
          <p className="text-xs font-medium mb-2 text-muted-foreground">
            Execution: Failed
          </p>
          <ReactFlowStoryWrapper
            nodes={[createStateNode('cond-failed', 'Failed')]}
            height={150}
            width={350}
          />
        </div>
      </div>
    );
  },
};
