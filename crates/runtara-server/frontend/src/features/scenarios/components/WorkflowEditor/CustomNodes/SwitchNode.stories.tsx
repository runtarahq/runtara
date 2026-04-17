import type { Meta, StoryObj } from '@storybook/react';
import { Node, Edge } from '@xyflow/react';
import { SwitchNode } from './SwitchNode';
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

const NODE_ID = 'switch-1';

// Helper to create a SwitchNode with specific configuration
interface CreateSwitchNodeOptions {
  cases?: Array<{
    matchType?: string;
    match?: any;
    output?: any;
    route?: string;
  }>;
  routingMode?: boolean;
  name?: string;
  selected?: boolean;
}

function createSwitchNode(opts: CreateSwitchNodeOptions = {}): Node {
  const {
    cases = [],
    routingMode,
    name = 'Order Router',
    selected = false,
  } = opts;

  const inputMapping: any[] = [
    { type: 'value', value: '{{input.region}}', typeHint: 'string' },
  ];

  if (cases.length > 0) {
    inputMapping.push({ type: 'cases', value: cases });
  }

  // Auto-detect routing mode if cases have routes, otherwise use explicit setting
  const hasRoutes = cases.some((c) => c.route);
  const isRouting = routingMode ?? hasRoutes;

  if (isRouting) {
    inputMapping.push({ type: 'routingMode', value: true });
  }

  inputMapping.push({ type: 'default', value: { fallback: true } });

  return {
    id: NODE_ID,
    type: NODE_TYPES.SwitchNode,
    position: { x: 100, y: 50 },
    selected,
    data: {
      id: NODE_ID,
      name,
      stepType: 'Switch',
      agentId: '',
      capabilityId: '',
      inputMapping,
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

const meta: Meta<typeof SwitchNode> = {
  title: 'WorkflowEditor/Nodes/SwitchNode',
  component: SwitchNode,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'Switch node for multi-way branching. Supports two modes: Value Mode (lookup table with single output) and Routing Mode (N-way branching with separate output handles per case).',
      },
    },
  },
  tags: ['autodocs'],
};

export default meta;
type Story = StoryObj<typeof SwitchNode>;

// Value Mode - single source handle, compact height
export const ValueMode: Story = {
  name: 'Value Mode (Single Output)',
  decorators: [
    (Story) => {
      resetStores();
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[createSwitchNode({ routingMode: false })]}
      height={200}
    />
  ),
};

// Routing Mode with 3 cases
export const RoutingThreeCases: Story = {
  name: 'Routing Mode (3 Cases)',
  decorators: [
    (Story) => {
      resetStores();
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[
        createSwitchNode({
          cases: [
            { matchType: 'exact', match: 'US', route: 'us_handler' },
            { matchType: 'exact', match: 'EU', route: 'eu_handler' },
            { matchType: 'exact', match: 'APAC', route: 'apac_handler' },
          ],
        }),
      ]}
      height={250}
    />
  ),
};

// Routing Mode with many cases (shows dynamic height)
export const RoutingManyCases: Story = {
  name: 'Routing Mode (8 Cases)',
  decorators: [
    (Story) => {
      resetStores();
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[
        createSwitchNode({
          cases: [
            { matchType: 'exact', match: 'US', route: 'us' },
            { matchType: 'exact', match: 'CA', route: 'ca' },
            { matchType: 'exact', match: 'MX', route: 'mx' },
            { matchType: 'exact', match: 'UK', route: 'uk' },
            { matchType: 'exact', match: 'DE', route: 'de' },
            { matchType: 'exact', match: 'FR', route: 'fr' },
            { matchType: 'exact', match: 'JP', route: 'jp' },
            { matchType: 'exact', match: 'AU', route: 'au' },
          ],
        }),
      ]}
      height={400}
    />
  ),
};

// Different match type labels
export const CaseLabelVariety: Story = {
  name: 'Case Label Variety',
  decorators: [
    (Story) => {
      resetStores();
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[
        createSwitchNode({
          name: 'Match Types Demo',
          cases: [
            { matchType: 'exact', match: 'US', route: 'exact' },
            { matchType: 'ne', match: 'XX', route: 'not_equals' },
            { matchType: 'in', match: ['A', 'B', 'C'], route: 'in_list' },
            { matchType: 'gte', match: 100, route: 'gte' },
            {
              matchType: 'range',
              match: { min: 1000, max: 5000 },
              route: 'range',
            },
            { matchType: 'starts_with', match: 'PRE_', route: 'prefix' },
            { matchType: 'is_defined', match: null, route: 'defined' },
          ],
        }),
      ]}
      height={350}
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
      nodes={[
        createSwitchNode({
          cases: [
            { matchType: 'exact', match: 'US', route: 'us' },
            { matchType: 'exact', match: 'EU', route: 'eu' },
          ],
          selected: true,
        }),
      ]}
      height={250}
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
      nodes={[
        createSwitchNode({
          cases: [
            { matchType: 'exact', match: 'US', route: 'us' },
            { matchType: 'exact', match: 'EU', route: 'eu' },
          ],
        }),
      ]}
      height={250}
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
        executionTime: 1250,
      });
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[
        createSwitchNode({
          cases: [
            { matchType: 'exact', match: 'US', route: 'us' },
            { matchType: 'exact', match: 'EU', route: 'eu' },
          ],
        }),
      ]}
      height={250}
    />
  ),
};

// Execution Failed
export const ExecutionFailed: Story = {
  name: 'Execution: Failed',
  decorators: [
    (Story) => {
      resetStores();
      setNodeExecutionStatus(NODE_ID, ExecutionStatus.Failed, {
        error: 'No matching case found for value: UNKNOWN',
      });
      return <Story />;
    },
  ],
  render: () => (
    <ReactFlowStoryWrapper
      nodes={[
        createSwitchNode({
          cases: [
            { matchType: 'exact', match: 'US', route: 'us' },
            { matchType: 'exact', match: 'EU', route: 'eu' },
          ],
        }),
      ]}
      height={250}
    />
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
    <ReactFlowStoryWrapper
      nodes={[
        createSwitchNode({
          cases: [{ matchType: 'exact', match: 'US', route: 'us' }],
        }),
      ]}
      height={200}
    />
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
    <ReactFlowStoryWrapper
      nodes={[
        createSwitchNode({
          cases: [{ matchType: 'exact', match: 'US', route: 'us' }],
        }),
      ]}
      height={200}
    />
  ),
};

// With connected edges (hides "+" buttons)
export const WithConnectedEdges: Story = {
  name: 'With Connected Edges',
  decorators: [
    (Story) => {
      resetStores();
      const edges: Edge[] = [
        {
          id: 'e1',
          source: NODE_ID,
          sourceHandle: 'case-0',
          target: 'target-1',
          targetHandle: 'target',
        },
        {
          id: 'e2',
          source: NODE_ID,
          sourceHandle: 'default',
          target: 'target-2',
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
        sourceHandle: 'case-0',
        target: 'target-1',
        targetHandle: 'target',
      },
      {
        id: 'e2',
        source: NODE_ID,
        sourceHandle: 'default',
        target: 'target-2',
        targetHandle: 'target',
      },
    ];
    return (
      <ReactFlowStoryWrapper
        nodes={[
          createSwitchNode({
            cases: [
              { matchType: 'exact', match: 'US', route: 'us' },
              { matchType: 'exact', match: 'EU', route: 'eu' },
            ],
          }),
          createTargetNode('target-1', 30),
          createTargetNode('target-2', 130),
        ]}
        edges={edges}
        width={600}
        height={250}
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
    <ReactFlowStoryWrapper
      nodes={[
        createSwitchNode({
          cases: [
            { matchType: 'exact', match: 'US', route: 'us' },
            { matchType: 'exact', match: 'EU', route: 'eu' },
          ],
        }),
      ]}
      height={250}
    />
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
      setNodeUnsaved('switch-unsaved');
      setNodeValidationError('switch-error');
      setNodeExecutionStatus('switch-running', ExecutionStatus.Running);
      setNodeExecutionStatus('switch-completed', ExecutionStatus.Completed, {
        executionTime: 1250,
      });
      setNodeExecutionStatus('switch-failed', ExecutionStatus.Failed, {
        error: 'Error message',
      });
      return <Story />;
    },
  ],
  render: () => {
    // Create nodes with unique IDs for each state
    const createStateNode = (id: string, name: string, y: number): Node => ({
      id,
      type: NODE_TYPES.SwitchNode,
      position: { x: 100, y },
      data: {
        id,
        name,
        stepType: 'Switch',
        agentId: '',
        capabilityId: '',
        inputMapping: [
          { type: 'value', value: '{{input.region}}', typeHint: 'string' },
          {
            type: 'cases',
            value: [
              { matchType: 'exact', match: 'US', route: 'us' },
              { matchType: 'exact', match: 'EU', route: 'eu' },
            ],
          },
          { type: 'routingMode', value: true },
          { type: 'default', value: {} },
        ],
      },
    });

    return (
      <div className="grid grid-cols-2 gap-4">
        <div>
          <p className="text-xs font-medium mb-2 text-muted-foreground">
            Default
          </p>
          <ReactFlowStoryWrapper
            nodes={[createStateNode('switch-default', 'Default', 50)]}
            height={200}
            width={350}
          />
        </div>
        <div>
          <p className="text-xs font-medium mb-2 text-muted-foreground">
            Unsaved Changes
          </p>
          <ReactFlowStoryWrapper
            nodes={[createStateNode('switch-unsaved', 'Unsaved', 50)]}
            height={200}
            width={350}
          />
        </div>
        <div>
          <p className="text-xs font-medium mb-2 text-muted-foreground">
            Validation Error
          </p>
          <ReactFlowStoryWrapper
            nodes={[createStateNode('switch-error', 'Error', 50)]}
            height={200}
            width={350}
          />
        </div>
        <div>
          <p className="text-xs font-medium mb-2 text-muted-foreground">
            Execution: Running
          </p>
          <ReactFlowStoryWrapper
            nodes={[createStateNode('switch-running', 'Running', 50)]}
            height={200}
            width={350}
          />
        </div>
        <div>
          <p className="text-xs font-medium mb-2 text-muted-foreground">
            Execution: Completed
          </p>
          <ReactFlowStoryWrapper
            nodes={[createStateNode('switch-completed', 'Completed', 50)]}
            height={200}
            width={350}
          />
        </div>
        <div>
          <p className="text-xs font-medium mb-2 text-muted-foreground">
            Execution: Failed
          </p>
          <ReactFlowStoryWrapper
            nodes={[createStateNode('switch-failed', 'Failed', 50)]}
            height={200}
            width={350}
          />
        </div>
      </div>
    );
  },
};
