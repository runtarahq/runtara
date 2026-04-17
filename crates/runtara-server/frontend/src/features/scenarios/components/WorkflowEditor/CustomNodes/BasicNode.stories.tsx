import type { Meta, StoryObj } from '@storybook/react';
import { Node, Edge } from '@xyflow/react';
import { BasicNode } from './BasicNode';
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
import { NODE_TYPES } from '@/features/scenarios/config/workflow';

const NODE_ID = 'basic-1';

// Helper to create a BasicNode with specific configuration
interface CreateBasicNodeOptions {
  stepType?: string;
  name?: string;
  agentId?: string;
  capabilityId?: string;
  selected?: boolean;
}

function createBasicNode(opts: CreateBasicNodeOptions = {}): Node {
  const {
    stepType = 'Agent',
    name = 'Process Order',
    agentId = '',
    capabilityId = '',
    selected = false,
  } = opts;

  return {
    id: NODE_ID,
    type: NODE_TYPES.BasicNode,
    position: { x: 100, y: 50 },
    selected,
    data: {
      id: NODE_ID,
      name,
      stepType,
      agentId,
      capabilityId,
      inputMapping: [],
    },
  };
}

// Create a target node for edge stories
function createTargetNode(id: string, y: number): Node {
  return {
    id,
    type: NODE_TYPES.BasicNode,
    position: { x: 400, y },
    data: {
      id,
      name: `Target ${id}`,
      stepType: 'Agent',
      agentId: '',
      capabilityId: '',
      inputMapping: [],
    },
  };
}

const meta: Meta<typeof BasicNode> = {
  title: 'WorkflowEditor/Nodes/BasicNode',
  component: BasicNode,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'Basic node used for most step types: Agent, Finish, StartScenario, etc. Has target, source, and optional onError handles.',
      },
    },
  },
  tags: ['autodocs'],
};

export default meta;
type Story = StoryObj<typeof BasicNode>;

// Default Agent step
export const Default: Story = {
  name: 'Default (Agent Step)',
  decorators: [
    (Story) => {
      resetStores();
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper nodes={[createBasicNode()]} height={200} />
  ),
};

// Finish step (no source handle)
export const FinishStep: Story = {
  name: 'Finish Step',
  decorators: [
    (Story) => {
      resetStores();
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[createBasicNode({ stepType: 'Finish', name: 'End Workflow' })]}
      height={200}
    />
  ),
};

// StartScenario step
export const StartScenarioStep: Story = {
  name: 'Start Scenario Step',
  decorators: [
    (Story) => {
      resetStores();
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[
        createBasicNode({
          stepType: 'StartScenario',
          name: 'Trigger Child Workflow',
        }),
      ]}
      height={200}
    />
  ),
};

// Agent with agent ID (would show agent name if agents were fetched)
export const AgentWithId: Story = {
  name: 'Agent with Agent ID',
  decorators: [
    (Story) => {
      resetStores();
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[
        createBasicNode({
          stepType: 'Agent',
          name: 'Shopify Sync',
          agentId: 'shopify-agent-001',
          capabilityId: 'sync-products',
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
      nodes={[createBasicNode({ selected: true })]}
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
    <ReactFlowStoryWrapper nodes={[createBasicNode()]} height={200} />
  ),
};

// Execution Completed
export const ExecutionCompleted: Story = {
  name: 'Execution: Completed',
  decorators: [
    (Story) => {
      resetStores();
      setNodeExecutionStatus(NODE_ID, ExecutionStatus.Completed, {
        executionTime: 532,
      });
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper nodes={[createBasicNode()]} height={200} />
  ),
};

// Execution Failed
export const ExecutionFailed: Story = {
  name: 'Execution: Failed',
  decorators: [
    (Story) => {
      resetStores();
      setNodeExecutionStatus(NODE_ID, ExecutionStatus.Failed, {
        error: 'Connection timeout to external API',
      });
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper nodes={[createBasicNode()]} height={200} />
  ),
};

// Execution Queued
export const ExecutionQueued: Story = {
  name: 'Execution: Queued',
  decorators: [
    (Story) => {
      resetStores();
      setNodeExecutionStatus(NODE_ID, ExecutionStatus.Queued);
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper nodes={[createBasicNode()]} height={200} />
  ),
};

// Unsaved changes indicator
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
    <ReactFlowStoryWrapper nodes={[createBasicNode()]} height={200} />
  ),
};

// Validation error indicator
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
    <ReactFlowStoryWrapper nodes={[createBasicNode()]} height={200} />
  ),
};

// With connected edges
export const WithConnectedEdges: Story = {
  name: 'With Connected Edges',
  decorators: [
    (Story) => {
      resetStores();
      const edges: Edge[] = [
        {
          id: 'e1',
          source: NODE_ID,
          sourceHandle: 'source',
          target: 'target-1',
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
        id: 'e1',
        source: NODE_ID,
        sourceHandle: 'source',
        target: 'target-1',
        targetHandle: 'target',
      },
    ];
    return (
      <ReactFlowStoryWrapper
        nodes={[createBasicNode(), createTargetNode('target-1', 50)]}
        edges={edges}
        width={600}
        height={200}
      />
    );
  },
};

// During scenario execution (read-only mode)
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
    <ReactFlowStoryWrapper nodes={[createBasicNode()]} height={200} />
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
      setNodeUnsaved('basic-unsaved');
      setNodeValidationError('basic-error');
      setNodeExecutionStatus('basic-running', ExecutionStatus.Running);
      setNodeExecutionStatus('basic-completed', ExecutionStatus.Completed, {
        executionTime: 532,
      });
      setNodeExecutionStatus('basic-failed', ExecutionStatus.Failed, {
        error: 'Error message',
      });
      setNodeExecutionStatus('basic-queued', ExecutionStatus.Queued);
      return <Story />;
    },
  ],
  render: () => {
    const createStateNode = (id: string, name: string, y: number): Node => ({
      id,
      type: NODE_TYPES.BasicNode,
      position: { x: 100, y },
      data: {
        id,
        name,
        stepType: 'Agent',
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
            nodes={[createStateNode('basic-default', 'Default', 50)]}
            height={150}
            width={350}
          />
        </div>
        <div>
          <p className="text-xs font-medium mb-2 text-muted-foreground">
            Unsaved Changes
          </p>
          <ReactFlowStoryWrapper
            nodes={[createStateNode('basic-unsaved', 'Unsaved', 50)]}
            height={150}
            width={350}
          />
        </div>
        <div>
          <p className="text-xs font-medium mb-2 text-muted-foreground">
            Validation Error
          </p>
          <ReactFlowStoryWrapper
            nodes={[createStateNode('basic-error', 'Error', 50)]}
            height={150}
            width={350}
          />
        </div>
        <div>
          <p className="text-xs font-medium mb-2 text-muted-foreground">
            Execution: Running
          </p>
          <ReactFlowStoryWrapper
            nodes={[createStateNode('basic-running', 'Running', 50)]}
            height={150}
            width={350}
          />
        </div>
        <div>
          <p className="text-xs font-medium mb-2 text-muted-foreground">
            Execution: Completed
          </p>
          <ReactFlowStoryWrapper
            nodes={[createStateNode('basic-completed', 'Completed', 50)]}
            height={150}
            width={350}
          />
        </div>
        <div>
          <p className="text-xs font-medium mb-2 text-muted-foreground">
            Execution: Failed
          </p>
          <ReactFlowStoryWrapper
            nodes={[createStateNode('basic-failed', 'Failed', 50)]}
            height={150}
            width={350}
          />
        </div>
      </div>
    );
  },
};

// Step types gallery
export const StepTypesGallery: Story = {
  name: 'Step Types Gallery',
  parameters: {
    layout: 'padded',
  },
  decorators: [
    (Story) => {
      resetStores();
      return <Story />;
    },
  ],
  render: () => {
    const stepTypes = [
      { type: 'Agent', name: 'Agent Step' },
      { type: 'Finish', name: 'Finish' },
      { type: 'StartScenario', name: 'Start Scenario' },
      { type: 'Start', name: 'Legacy Start' },
    ];

    return (
      <div className="grid grid-cols-2 gap-4">
        {stepTypes.map(({ type, name }) => (
          <div key={type}>
            <p className="text-xs font-medium mb-2 text-muted-foreground">
              {type}
            </p>
            <ReactFlowStoryWrapper
              nodes={[
                {
                  id: `basic-${type}`,
                  type: NODE_TYPES.BasicNode,
                  position: { x: 100, y: 50 },
                  data: {
                    id: `basic-${type}`,
                    name,
                    stepType: type,
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
        ))}
      </div>
    );
  },
};
