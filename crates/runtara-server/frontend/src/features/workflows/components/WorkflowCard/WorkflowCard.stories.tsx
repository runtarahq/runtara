import type { Meta, StoryObj } from '@storybook/react';
import { fn } from '@storybook/test';
import { WorkflowCard } from './index';
import { WorkflowDto } from '@/generated/RuntaraRuntimeApi';

const meta: Meta<typeof WorkflowCard> = {
  title: 'Workflows/WorkflowCard',
  component: WorkflowCard,
  parameters: {
    layout: 'padded',
    docs: {
      description: {
        component:
          'A card component for displaying workflow information with actions for running, editing, cloning, and deleting. Shows version, timestamps, and input indicators.',
      },
    },
  },
  tags: ['autodocs'],
  argTypes: {
    pendingActionType: {
      control: 'select',
      options: [undefined, 'schedule', 'clone', 'delete', 'move'],
      description: 'Type of pending action (shows loading state)',
    },
    showMoveAction: {
      control: 'boolean',
      description: 'Whether to show the move to folder button',
    },
  },
  args: {
    onUpdate: fn(),
    onDelete: fn(),
    onSchedule: fn(),
    onClone: fn(),
    onMoveToFolder: fn(),
  },
};

export default meta;
type Story = StoryObj<typeof WorkflowCard>;

// Sample workflow data
const baseWorkflow: WorkflowDto = {
  id: 'scn-001',
  name: 'Order Processing Workflow',
  description:
    'Processes incoming orders from the storefront and updates inventory automatically.',
  currentVersionNumber: 3,
  lastVersionNumber: 3,
  created: '2024-01-15T10:30:00Z',
  updated: '2024-01-25T14:45:00Z',
  executionGraph: {},
  inputSchema: null,
  outputSchema: null,
};

const workflowWithInputs: WorkflowDto = {
  ...baseWorkflow,
  id: 'scn-002',
  name: 'Customer Data Sync',
  description:
    'Synchronizes customer data between CRM and e-commerce platform.',
  inputSchema: {
    type: 'object',
    properties: {
      customerId: { type: 'string' },
    },
  },
};

const draftWorkflow: WorkflowDto = {
  id: 'scn-003',
  name: 'New Workflow (Draft)',
  description: 'Work in progress workflow.',
  currentVersionNumber: 0,
  lastVersionNumber: 0,
  created: '2024-01-25T08:00:00Z',
  updated: '2024-01-25T08:00:00Z',
  executionGraph: {},
  inputSchema: null,
  outputSchema: null,
};

const longDescriptionWorkflow: WorkflowDto = {
  ...baseWorkflow,
  id: 'scn-004',
  name: 'Complex Integration Workflow with a Very Long Name',
  description:
    'This is a very long description that explains in detail what this workflow does. It integrates multiple systems, handles error cases, and provides comprehensive logging and monitoring capabilities.',
};

export const Default: Story = {
  args: {
    workflow: baseWorkflow,
  },
};

export const WithInputs: Story = {
  name: 'With Input Schema',
  args: {
    workflow: workflowWithInputs,
  },
  parameters: {
    docs: {
      description: {
        story:
          'Workflows with input schemas show an "Inputs" badge to indicate they require input parameters.',
      },
    },
  },
};

export const Draft: Story = {
  args: {
    workflow: draftWorkflow,
  },
};

export const LongContent: Story = {
  name: 'Long Content',
  args: {
    workflow: longDescriptionWorkflow,
  },
};

export const LoadingSchedule: Story = {
  name: 'Loading (Schedule)',
  args: {
    workflow: baseWorkflow,
    pendingActionId: 'scn-001',
    pendingActionType: 'schedule',
  },
};

export const LoadingClone: Story = {
  name: 'Loading (Clone)',
  args: {
    workflow: baseWorkflow,
    pendingActionId: 'scn-001',
    pendingActionType: 'clone',
  },
};

export const LoadingDelete: Story = {
  name: 'Loading (Delete)',
  args: {
    workflow: baseWorkflow,
    pendingActionId: 'scn-001',
    pendingActionType: 'delete',
  },
};

export const WithMoveAction: Story = {
  name: 'With Move Action',
  args: {
    workflow: baseWorkflow,
    showMoveAction: true,
  },
};

export const LoadingMove: Story = {
  name: 'Loading (Move)',
  args: {
    workflow: baseWorkflow,
    showMoveAction: true,
    pendingActionId: 'scn-001',
    pendingActionType: 'move',
  },
};

export const GridLayout: Story = {
  name: 'Grid Layout',
  render: (args) => (
    <div className="grid grid-cols-2 gap-4 max-w-4xl">
      <WorkflowCard {...args} workflow={baseWorkflow} />
      <WorkflowCard {...args} workflow={workflowWithInputs} />
      <WorkflowCard {...args} workflow={draftWorkflow} />
      <WorkflowCard
        {...args}
        workflow={{
          ...baseWorkflow,
          id: 'scn-005',
          name: 'Inventory Check',
          description: 'Performs daily inventory reconciliation.',
          currentVersionNumber: 1,
        }}
      />
    </div>
  ),
};

export const AllStates: Story = {
  name: 'All States Reference',
  render: (args) => (
    <div className="space-y-4 max-w-xl">
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          Standard
        </p>
        <WorkflowCard {...args} workflow={baseWorkflow} />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          With Inputs Badge
        </p>
        <WorkflowCard {...args} workflow={workflowWithInputs} />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          Draft (No Version)
        </p>
        <WorkflowCard {...args} workflow={draftWorkflow} />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          With Move Action
        </p>
        <WorkflowCard {...args} workflow={baseWorkflow} showMoveAction />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          Loading States
        </p>
        <div className="space-y-2">
          <WorkflowCard
            {...args}
            workflow={{
              ...baseWorkflow,
              id: 'loading-1',
              name: 'Scheduling...',
            }}
            pendingActionId="loading-1"
            pendingActionType="schedule"
          />
        </div>
      </div>
    </div>
  ),
};
