import type { Meta, StoryObj } from '@storybook/react';
import { Badge } from './badge';
import { Check, X, AlertTriangle, Clock, Info, Star } from 'lucide-react';

const meta: Meta<typeof Badge> = {
  title: 'UI/Badge',
  component: Badge,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'A small label component for displaying status, categories, or tags. Built with CVA for consistent variant styling.',
      },
    },
  },
  tags: ['autodocs'],
  argTypes: {
    variant: {
      control: 'select',
      options: [
        'default',
        'secondary',
        'destructive',
        'outline',
        'success',
        'warning',
        'muted',
      ],
      description: 'Visual style variant',
    },
    children: {
      control: 'text',
      description: 'Badge content',
    },
  },
  args: {
    children: 'Badge',
  },
};

export default meta;
type Story = StoryObj<typeof Badge>;

export const Default: Story = {
  args: {
    variant: 'default',
    children: 'Default',
  },
};

export const Secondary: Story = {
  args: {
    variant: 'secondary',
    children: 'Secondary',
  },
};

export const Destructive: Story = {
  args: {
    variant: 'destructive',
    children: 'Destructive',
  },
};

export const Outline: Story = {
  args: {
    variant: 'outline',
    children: 'Outline',
  },
};

export const Success: Story = {
  args: {
    variant: 'success',
    children: 'Success',
  },
};

export const Warning: Story = {
  args: {
    variant: 'warning',
    children: 'Warning',
  },
};

export const Muted: Story = {
  args: {
    variant: 'muted',
    children: 'Muted',
  },
};

export const AllVariants: Story = {
  name: 'All Variants',
  render: () => (
    <div className="flex flex-wrap gap-2">
      <Badge variant="default">Default</Badge>
      <Badge variant="secondary">Secondary</Badge>
      <Badge variant="destructive">Destructive</Badge>
      <Badge variant="outline">Outline</Badge>
      <Badge variant="success">Success</Badge>
      <Badge variant="warning">Warning</Badge>
      <Badge variant="muted">Muted</Badge>
    </div>
  ),
};

export const WithIcons: Story = {
  name: 'With Icons',
  render: () => (
    <div className="flex flex-wrap gap-2">
      <Badge variant="success" className="gap-1">
        <Check className="h-3 w-3" />
        Approved
      </Badge>
      <Badge variant="destructive" className="gap-1">
        <X className="h-3 w-3" />
        Rejected
      </Badge>
      <Badge variant="warning" className="gap-1">
        <AlertTriangle className="h-3 w-3" />
        Warning
      </Badge>
      <Badge variant="muted" className="gap-1">
        <Clock className="h-3 w-3" />
        Pending
      </Badge>
      <Badge variant="default" className="gap-1">
        <Info className="h-3 w-3" />
        Info
      </Badge>
      <Badge variant="secondary" className="gap-1">
        <Star className="h-3 w-3" />
        Featured
      </Badge>
    </div>
  ),
};

export const StatusBadges: Story = {
  name: 'Status Use Cases',
  render: () => (
    <div className="space-y-4">
      <div>
        <p className="text-sm font-medium mb-2">Execution Status</p>
        <div className="flex flex-wrap gap-2">
          <Badge variant="success">Completed</Badge>
          <Badge variant="warning">Running</Badge>
          <Badge variant="destructive">Failed</Badge>
          <Badge variant="muted">Pending</Badge>
          <Badge variant="secondary">Scheduled</Badge>
        </div>
      </div>
      <div>
        <p className="text-sm font-medium mb-2">Connection Status</p>
        <div className="flex flex-wrap gap-2">
          <Badge variant="success">Connected</Badge>
          <Badge variant="destructive">Disconnected</Badge>
          <Badge variant="warning">Reconnecting</Badge>
        </div>
      </div>
      <div>
        <p className="text-sm font-medium mb-2">Priority Levels</p>
        <div className="flex flex-wrap gap-2">
          <Badge variant="destructive">Critical</Badge>
          <Badge variant="warning">High</Badge>
          <Badge variant="default">Medium</Badge>
          <Badge variant="muted">Low</Badge>
        </div>
      </div>
    </div>
  ),
};

export const CountBadges: Story = {
  name: 'Count Badges',
  render: () => (
    <div className="flex flex-wrap gap-4">
      <div className="flex items-center gap-2">
        <span className="text-sm">Notifications</span>
        <Badge variant="destructive">12</Badge>
      </div>
      <div className="flex items-center gap-2">
        <span className="text-sm">Messages</span>
        <Badge variant="default">5</Badge>
      </div>
      <div className="flex items-center gap-2">
        <span className="text-sm">Tasks</span>
        <Badge variant="muted">24</Badge>
      </div>
    </div>
  ),
};

export const LongContent: Story = {
  name: 'Long Content',
  render: () => (
    <div className="space-y-2 max-w-md">
      <Badge variant="default">Short</Badge>
      <Badge variant="default">Medium length badge</Badge>
      <Badge variant="default">
        This is a very long badge content that might wrap
      </Badge>
    </div>
  ),
};
