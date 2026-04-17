import type { Meta, StoryObj } from '@storybook/react';
import { useState } from 'react';
import { DateRangeSelector, DateRangeOption } from './date-range-selector';

const meta: Meta<typeof DateRangeSelector> = {
  title: 'Shared/DateRangeSelector',
  component: DateRangeSelector,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'A select dropdown for choosing predefined date ranges (Last Hour, 24 Hours, 7 Days, 30 Days, 90 Days). Used in analytics dashboards.',
      },
    },
  },
  tags: ['autodocs'],
  argTypes: {
    value: {
      control: 'select',
      options: ['1h', '24h', '7d', '30d', '90d'],
      description: 'Currently selected date range',
    },
  },
};

export default meta;
type Story = StoryObj<typeof DateRangeSelector>;

export const Default: Story = {
  args: {
    value: '24h',
    onChange: () => {},
  },
};

export const LastHour: Story = {
  name: 'Last Hour',
  args: {
    value: '1h',
    onChange: () => {},
  },
};

export const Last7Days: Story = {
  name: 'Last 7 Days',
  args: {
    value: '7d',
    onChange: () => {},
  },
};

export const Last30Days: Story = {
  name: 'Last 30 Days',
  args: {
    value: '30d',
    onChange: () => {},
  },
};

export const Last90Days: Story = {
  name: 'Last 90 Days',
  args: {
    value: '90d',
    onChange: () => {},
  },
};

// Interactive example
const InteractiveExample = () => {
  const [value, setValue] = useState<DateRangeOption>('24h');

  return (
    <div className="space-y-4">
      <DateRangeSelector value={value} onChange={setValue} />
      <div className="p-3 bg-muted rounded text-sm">
        <strong>Selected:</strong> {value}
      </div>
    </div>
  );
};

export const Interactive: Story = {
  render: () => <InteractiveExample />,
};

export const AllOptions: Story = {
  name: 'All Options Reference',
  render: () => (
    <div className="space-y-4">
      <div className="text-sm">
        <p className="font-medium mb-2">Available Options:</p>
        <ul className="text-muted-foreground text-xs space-y-1">
          <li>
            <code className="bg-muted px-1 rounded">1h</code> - Last Hour
          </li>
          <li>
            <code className="bg-muted px-1 rounded">24h</code> - Last 24 Hours
          </li>
          <li>
            <code className="bg-muted px-1 rounded">7d</code> - Last 7 Days
          </li>
          <li>
            <code className="bg-muted px-1 rounded">30d</code> - Last 30 Days
          </li>
          <li>
            <code className="bg-muted px-1 rounded">90d</code> - Last 90 Days
          </li>
        </ul>
      </div>
      <DateRangeSelector value="24h" onChange={() => {}} />
    </div>
  ),
};
