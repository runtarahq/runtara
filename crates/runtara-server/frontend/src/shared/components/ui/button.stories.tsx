import type { Meta, StoryObj } from '@storybook/react';
import { fn } from '@storybook/test';
import { Mail, Loader2, ChevronRight, Plus, Trash2 } from 'lucide-react';
import { Button } from './button';

const meta: Meta<typeof Button> = {
  title: 'UI/Button',
  component: Button,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'A versatile button component with multiple variants and sizes. Built on Radix UI Slot for composition.',
      },
    },
  },
  tags: ['autodocs'],
  argTypes: {
    variant: {
      control: 'select',
      options: [
        'default',
        'destructive',
        'outline',
        'secondary',
        'ghost',
        'link',
      ],
      description: 'The visual style variant of the button',
    },
    size: {
      control: 'select',
      options: ['default', 'sm', 'lg', 'icon'],
      description: 'The size of the button',
    },
    asChild: {
      control: 'boolean',
      description: 'Render as child element (uses Radix Slot)',
    },
    disabled: {
      control: 'boolean',
    },
  },
  args: {
    onClick: fn(),
  },
};

export default meta;
type Story = StoryObj<typeof Button>;

export const Default: Story = {
  args: {
    children: 'Button',
    variant: 'default',
    size: 'default',
  },
};

export const AllVariants: Story = {
  name: 'All Variants',
  render: () => (
    <div className="flex flex-wrap gap-4">
      <Button variant="default">Default</Button>
      <Button variant="destructive">Destructive</Button>
      <Button variant="outline">Outline</Button>
      <Button variant="secondary">Secondary</Button>
      <Button variant="ghost">Ghost</Button>
      <Button variant="link">Link</Button>
    </div>
  ),
};

export const AllSizes: Story = {
  name: 'All Sizes',
  render: () => (
    <div className="flex items-center gap-4">
      <Button size="sm">Small</Button>
      <Button size="default">Default</Button>
      <Button size="lg">Large</Button>
      <Button size="icon">
        <Plus className="h-4 w-4" />
      </Button>
    </div>
  ),
};

export const WithLeftIcon: Story = {
  name: 'With Left Icon',
  args: {
    children: (
      <>
        <Mail className="h-4 w-4" />
        Send Email
      </>
    ),
  },
};

export const WithRightIcon: Story = {
  name: 'With Right Icon',
  args: {
    children: (
      <>
        Next
        <ChevronRight className="h-4 w-4" />
      </>
    ),
  },
};

export const Loading: Story = {
  args: {
    disabled: true,
    children: (
      <>
        <Loader2 className="h-4 w-4 animate-spin" />
        Please wait
      </>
    ),
  },
};

export const Disabled: Story = {
  args: {
    children: 'Disabled',
    disabled: true,
  },
};

export const Destructive: Story = {
  args: {
    children: (
      <>
        <Trash2 className="h-4 w-4" />
        Delete
      </>
    ),
    variant: 'destructive',
  },
};

export const AsLink: Story = {
  name: 'As Link',
  render: () => (
    <Button asChild variant="link">
      <a href="https://example.com">Visit Website</a>
    </Button>
  ),
};

export const IconButton: Story = {
  name: 'Icon Button',
  render: () => (
    <div className="flex gap-2">
      <Button size="icon" variant="default">
        <Plus className="h-4 w-4" />
      </Button>
      <Button size="icon" variant="outline">
        <Mail className="h-4 w-4" />
      </Button>
      <Button size="icon" variant="ghost">
        <Trash2 className="h-4 w-4" />
      </Button>
    </div>
  ),
};
