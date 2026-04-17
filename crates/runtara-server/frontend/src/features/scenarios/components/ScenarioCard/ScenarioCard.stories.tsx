import type { Meta, StoryObj } from '@storybook/react';
import { fn } from '@storybook/test';
import { ScenarioCard } from './index';
import { ScenarioDto } from '@/generated/RuntaraRuntimeApi';

const meta: Meta<typeof ScenarioCard> = {
  title: 'Scenarios/ScenarioCard',
  component: ScenarioCard,
  parameters: {
    layout: 'padded',
    docs: {
      description: {
        component:
          'A card component for displaying scenario information with actions for running, editing, cloning, and deleting. Shows version, timestamps, and input indicators.',
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
type Story = StoryObj<typeof ScenarioCard>;

// Sample scenario data
const baseScenario: ScenarioDto = {
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

const scenarioWithInputs: ScenarioDto = {
  ...baseScenario,
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

const draftScenario: ScenarioDto = {
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

const longDescriptionScenario: ScenarioDto = {
  ...baseScenario,
  id: 'scn-004',
  name: 'Complex Integration Scenario with a Very Long Name',
  description:
    'This is a very long description that explains in detail what this scenario does. It integrates multiple systems, handles error cases, and provides comprehensive logging and monitoring capabilities.',
};

export const Default: Story = {
  args: {
    scenario: baseScenario,
  },
};

export const WithInputs: Story = {
  name: 'With Input Schema',
  args: {
    scenario: scenarioWithInputs,
  },
  parameters: {
    docs: {
      description: {
        story:
          'Scenarios with input schemas show an "Inputs" badge to indicate they require input parameters.',
      },
    },
  },
};

export const Draft: Story = {
  args: {
    scenario: draftScenario,
  },
};

export const LongContent: Story = {
  name: 'Long Content',
  args: {
    scenario: longDescriptionScenario,
  },
};

export const LoadingSchedule: Story = {
  name: 'Loading (Schedule)',
  args: {
    scenario: baseScenario,
    pendingActionId: 'scn-001',
    pendingActionType: 'schedule',
  },
};

export const LoadingClone: Story = {
  name: 'Loading (Clone)',
  args: {
    scenario: baseScenario,
    pendingActionId: 'scn-001',
    pendingActionType: 'clone',
  },
};

export const LoadingDelete: Story = {
  name: 'Loading (Delete)',
  args: {
    scenario: baseScenario,
    pendingActionId: 'scn-001',
    pendingActionType: 'delete',
  },
};

export const WithMoveAction: Story = {
  name: 'With Move Action',
  args: {
    scenario: baseScenario,
    showMoveAction: true,
  },
};

export const LoadingMove: Story = {
  name: 'Loading (Move)',
  args: {
    scenario: baseScenario,
    showMoveAction: true,
    pendingActionId: 'scn-001',
    pendingActionType: 'move',
  },
};

export const GridLayout: Story = {
  name: 'Grid Layout',
  render: (args) => (
    <div className="grid grid-cols-2 gap-4 max-w-4xl">
      <ScenarioCard {...args} scenario={baseScenario} />
      <ScenarioCard {...args} scenario={scenarioWithInputs} />
      <ScenarioCard {...args} scenario={draftScenario} />
      <ScenarioCard
        {...args}
        scenario={{
          ...baseScenario,
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
        <ScenarioCard {...args} scenario={baseScenario} />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          With Inputs Badge
        </p>
        <ScenarioCard {...args} scenario={scenarioWithInputs} />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          Draft (No Version)
        </p>
        <ScenarioCard {...args} scenario={draftScenario} />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          With Move Action
        </p>
        <ScenarioCard {...args} scenario={baseScenario} showMoveAction />
      </div>
      <div>
        <p className="text-xs font-medium mb-2 text-muted-foreground">
          Loading States
        </p>
        <div className="space-y-2">
          <ScenarioCard
            {...args}
            scenario={{
              ...baseScenario,
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
