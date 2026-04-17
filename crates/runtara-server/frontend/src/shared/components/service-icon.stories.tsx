import type { Meta, StoryObj } from '@storybook/react';
import { ServiceIcon } from './service-icon';

const meta: Meta<typeof ServiceIcon> = {
  title: 'Shared/ServiceIcon',
  component: ServiceIcon,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'An icon component that displays service/integration icons with gradient backgrounds. Automatically selects icons based on service ID or category.',
      },
    },
  },
  tags: ['autodocs'],
  argTypes: {
    serviceId: {
      control: 'text',
      description: 'Service identifier (e.g., "shopify", "mysql", "aws")',
    },
    category: {
      control: 'select',
      options: [
        'file_storage',
        'database',
        'email',
        'cloud',
        'api',
        'ecommerce',
        'payment',
        'messaging',
      ],
      description: 'Service category for fallback icon selection',
    },
    className: {
      control: 'text',
      description: 'CSS classes for sizing (default: "w-10 h-10")',
    },
  },
};

export default meta;
type Story = StoryObj<typeof ServiceIcon>;

export const Default: Story = {
  args: {
    serviceId: 'shopify',
  },
};

export const Database: Story = {
  args: {
    serviceId: 'mysql',
  },
};

export const Cloud: Story = {
  args: {
    serviceId: 'aws',
  },
};

export const Email: Story = {
  args: {
    serviceId: 'smtp',
  },
};

export const Payment: Story = {
  args: {
    serviceId: 'stripe',
  },
};

export const ByCategory: Story = {
  name: 'By Category (Fallback)',
  args: {
    category: 'database',
  },
};

export const CustomSize: Story = {
  name: 'Custom Size',
  args: {
    serviceId: 'shopify',
    className: 'w-16 h-16',
  },
};

export const SmallSize: Story = {
  name: 'Small Size',
  args: {
    serviceId: 'mysql',
    className: 'w-6 h-6',
  },
};

export const UnknownService: Story = {
  name: 'Unknown Service (Fallback)',
  args: {
    serviceId: 'unknown-service',
  },
};

export const AllServices: Story = {
  name: 'All Services Reference',
  render: () => (
    <div className="grid grid-cols-4 gap-6">
      {[
        'sftp',
        'mysql',
        'mongodb',
        'smtp',
        'aws',
        'azure',
        'http',
        'shopify',
        'stripe',
        'slack',
        'webhook',
        'csv',
      ].map((id) => (
        <div key={id} className="flex flex-col items-center gap-2">
          <ServiceIcon serviceId={id} />
          <span className="text-xs text-muted-foreground">{id}</span>
        </div>
      ))}
    </div>
  ),
};

export const AllCategories: Story = {
  name: 'All Categories Reference',
  render: () => (
    <div className="grid grid-cols-4 gap-6">
      {[
        'file_storage',
        'database',
        'email',
        'cloud',
        'api',
        'ecommerce',
        'payment',
        'messaging',
      ].map((cat) => (
        <div key={cat} className="flex flex-col items-center gap-2">
          <ServiceIcon category={cat} />
          <span className="text-xs text-muted-foreground">{cat}</span>
        </div>
      ))}
    </div>
  ),
};

export const SizeComparison: Story = {
  name: 'Size Comparison',
  render: () => (
    <div className="flex items-end gap-4">
      <div className="flex flex-col items-center gap-2">
        <ServiceIcon serviceId="shopify" className="w-6 h-6" />
        <span className="text-xs text-muted-foreground">w-6</span>
      </div>
      <div className="flex flex-col items-center gap-2">
        <ServiceIcon serviceId="shopify" className="w-10 h-10" />
        <span className="text-xs text-muted-foreground">w-10</span>
      </div>
      <div className="flex flex-col items-center gap-2">
        <ServiceIcon serviceId="shopify" className="w-16 h-16" />
        <span className="text-xs text-muted-foreground">w-16</span>
      </div>
      <div className="flex flex-col items-center gap-2">
        <ServiceIcon serviceId="shopify" className="w-24 h-24" />
        <span className="text-xs text-muted-foreground">w-24</span>
      </div>
    </div>
  ),
};
