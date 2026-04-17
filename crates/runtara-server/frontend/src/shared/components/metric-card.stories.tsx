import type { Meta, StoryObj } from '@storybook/react';
import { MetricCard } from './metric-card';

const meta: Meta<typeof MetricCard> = {
  title: 'Shared/MetricCard',
  component: MetricCard,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'A card component for displaying metrics with optional trend indicators. Automatically colors trends based on metric type (success rate vs error count vs duration).',
      },
    },
  },
  tags: ['autodocs'],
  argTypes: {
    title: {
      control: 'text',
      description: 'Metric title',
    },
    value: {
      control: 'text',
      description: 'Metric value (string or number)',
    },
    change: {
      control: 'number',
      description: 'Percentage change from previous period',
    },
    trend: {
      control: 'select',
      options: [undefined, 'up', 'down', 'stable'],
      description: 'Trend direction',
    },
    loading: {
      control: 'boolean',
      description: 'Show loading skeleton',
    },
  },
};

export default meta;
type Story = StoryObj<typeof MetricCard>;

export const Default: Story = {
  args: {
    title: 'Total Users',
    value: 12345,
  },
};

export const WithTrendUp: Story = {
  name: 'With Trend (Up)',
  args: {
    title: 'Total Users',
    value: 15678,
    change: 23.5,
    trend: 'up',
  },
};

export const WithTrendDown: Story = {
  name: 'With Trend (Down)',
  args: {
    title: 'Total Users',
    value: 8432,
    change: 12.3,
    trend: 'down',
  },
};

export const SuccessRate: Story = {
  name: 'Success Rate (Up is Good)',
  args: {
    title: 'Success Rate',
    value: '98.5%',
    change: 2.3,
    trend: 'up',
  },
};

export const SuccessRateDown: Story = {
  name: 'Success Rate (Down is Bad)',
  args: {
    title: 'Success Rate',
    value: '95.2%',
    change: 3.5,
    trend: 'down',
  },
};

export const ErrorCount: Story = {
  name: 'Error Count (Up is Bad)',
  args: {
    title: 'Error Count',
    value: 47,
    change: 15.2,
    trend: 'up',
  },
};

export const ErrorCountDown: Story = {
  name: 'Error Count (Down is Good)',
  args: {
    title: 'Error Count',
    value: 12,
    change: 45.8,
    trend: 'down',
  },
};

export const Duration: Story = {
  name: 'Avg Duration (Up is Bad)',
  args: {
    title: 'Avg Duration',
    value: '2.3s',
    change: 18.5,
    trend: 'up',
  },
};

export const DurationDown: Story = {
  name: 'Avg Duration (Down is Good)',
  args: {
    title: 'Avg Duration',
    value: '1.2s',
    change: 25.0,
    trend: 'down',
  },
};

export const Loading: Story = {
  args: {
    title: 'Loading Metric',
    value: 0,
    loading: true,
  },
};

export const LargeNumber: Story = {
  name: 'Large Number',
  args: {
    title: 'API Calls',
    value: 1234567,
    change: 8.2,
    trend: 'up',
  },
};

export const StringValue: Story = {
  name: 'String Value',
  args: {
    title: 'Uptime',
    value: '99.99%',
    change: 0.01,
    trend: 'up',
  },
};

export const NoChange: Story = {
  name: 'No Change Data',
  args: {
    title: 'Active Users',
    value: 42,
  },
};

export const GridLayout: Story = {
  name: 'Grid Layout',
  render: () => (
    <div className="grid grid-cols-2 gap-4 w-[600px]">
      <MetricCard title="Total Users" value={15678} change={23.5} trend="up" />
      <MetricCard title="Success Rate" value="98.5%" change={2.1} trend="up" />
      <MetricCard title="Error Count" value={47} change={12.3} trend="down" />
      <MetricCard
        title="Avg Duration"
        value="1.8s"
        change={15.2}
        trend="down"
      />
    </div>
  ),
};

export const AllStates: Story = {
  name: 'All States Reference',
  render: () => (
    <div className="space-y-4 w-[300px]">
      <MetricCard title="Default" value={1234} />
      <MetricCard title="Trend Up" value={5678} change={12.5} trend="up" />
      <MetricCard title="Trend Down" value={910} change={8.3} trend="down" />
      <MetricCard title="Loading" value={0} loading />
    </div>
  ),
};
