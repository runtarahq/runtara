import type { Meta, StoryObj } from '@storybook/react';
import { EntityTile } from './entity-tile';
import { Button } from './ui/button';
import { Badge } from './ui/badge';
import {
  Play,
  Pencil,
  Copy,
  Trash2,
  Clock,
  Calendar,
  Tag,
  Star,
  AlertCircle,
  ExternalLink,
} from 'lucide-react';

const meta: Meta<typeof EntityTile> = {
  title: 'Layout/EntityTile',
  component: EntityTile,
  parameters: {
    layout: 'padded',
    docs: {
      description: {
        component:
          'A flexible card component for displaying entity information with title, description, metadata, badges, and action buttons. Used for workflows, connections, and other list items.',
      },
    },
  },
  tags: ['autodocs'],
  argTypes: {
    kicker: {
      control: 'text',
      description: 'Small label shown before the title (e.g., version number)',
    },
    title: {
      control: 'text',
      description: 'Main title of the entity',
    },
    description: {
      control: 'text',
      description: 'Description text below the title',
    },
    showDivider: {
      control: 'boolean',
      description: 'Show divider above footer',
    },
  },
};

export default meta;
type Story = StoryObj<typeof EntityTile>;

export const Default: Story = {
  args: {
    kicker: 'v1',
    title: 'Order Processing Workflow',
    description:
      'Processes incoming orders and updates inventory automatically.',
  },
};

export const WithMetadata: Story = {
  name: 'With Metadata',
  render: () => (
    <EntityTile
      kicker="v2"
      title="Customer Sync Workflow"
      description="Synchronizes customer data between systems every 15 minutes."
      metadata={[
        <>
          <Clock className="w-3.5 h-3.5" />2 hours ago
        </>,
        <>
          <Calendar className="w-3.5 h-3.5" />
          Jan 15, 2024
        </>,
      ]}
    />
  ),
};

export const WithActions: Story = {
  name: 'With Actions',
  render: () => (
    <EntityTile
      kicker="v3"
      title="Inventory Update"
      description="Updates stock levels based on incoming shipments."
      actions={
        <>
          <Button
            variant="ghost"
            size="icon"
            className="p-2 h-auto w-auto text-slate-400 hover:text-emerald-600 hover:bg-emerald-50"
          >
            <Play className="w-4 h-4" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="p-2 h-auto w-auto text-slate-400 hover:text-blue-600 hover:bg-blue-50"
          >
            <Pencil className="w-4 h-4" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="p-2 h-auto w-auto text-slate-400 hover:text-slate-600 hover:bg-slate-100"
          >
            <Copy className="w-4 h-4" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            className="p-2 h-auto w-auto text-slate-400 hover:text-red-600 hover:bg-red-50"
          >
            <Trash2 className="w-4 h-4" />
          </Button>
        </>
      }
    />
  ),
  parameters: {
    docs: {
      description: {
        story: 'Hover over the card to see action buttons appear.',
      },
    },
  },
};

export const WithBadges: Story = {
  name: 'With Badges',
  render: () => (
    <EntityTile
      kicker="v1"
      title="Order Processing"
      description="Handles all incoming orders from the storefront."
      badges={
        <div className="flex gap-1">
          <Badge variant="warning" className="text-[10px] px-1.5 py-0.5">
            <AlertCircle className="w-2.5 h-2.5 mr-0.5" />
            Inputs
          </Badge>
          <Badge variant="success" className="text-[10px] px-1.5 py-0.5">
            Active
          </Badge>
        </div>
      }
    />
  ),
};

export const WithTags: Story = {
  name: 'With Tags',
  render: () => (
    <EntityTile
      kicker="v2"
      title="Product Import"
      description="Imports products from external suppliers."
      metadata={[
        <>
          <Clock className="w-3.5 h-3.5" />5 min ago
        </>,
      ]}
      tags={
        <div className="flex gap-1 overflow-hidden">
          {['orders', 'inventory', 'sync'].map((tag) => (
            <span
              key={tag}
              className="inline-flex items-center gap-1 px-1.5 py-0.5 text-[10px] bg-slate-100 text-slate-600 rounded"
            >
              <Tag className="w-2.5 h-2.5" />
              {tag}
            </span>
          ))}
        </div>
      }
    />
  ),
};

export const WithFooter: Story = {
  name: 'With Footer',
  render: () => (
    <EntityTile
      kicker="v1"
      title="Scheduled Report"
      description="Generates daily sales reports."
      showDivider
      footer={
        <div className="flex items-center justify-between">
          <span className="text-xs text-muted-foreground">
            Next run: Tomorrow 8:00 AM
          </span>
          <Button variant="outline" size="sm">
            View History
          </Button>
        </div>
      }
    />
  ),
};

export const FullFeatured: Story = {
  name: 'Full Featured',
  render: () => (
    <EntityTile
      kicker="v4"
      title="Complete Workflow Example"
      description="This demonstrates all the features of the EntityTile component including badges, metadata, tags, actions, and footer."
      badges={
        <div className="flex gap-1">
          <Badge variant="success" className="text-[10px]">
            Production
          </Badge>
          <Badge variant="warning" className="text-[10px]">
            <Star className="w-2.5 h-2.5 mr-0.5" />
            Featured
          </Badge>
        </div>
      }
      metadata={[
        <>
          <Clock className="w-3.5 h-3.5" />
          Updated 30 min ago
        </>,
        <>
          <Calendar className="w-3.5 h-3.5" />
          Created Jan 10, 2024
        </>,
      ]}
      tags={
        <div className="flex gap-1">
          {['automation', 'critical'].map((tag) => (
            <span
              key={tag}
              className="inline-flex items-center gap-1 px-1.5 py-0.5 text-[10px] bg-primary/10 text-primary rounded"
            >
              {tag}
            </span>
          ))}
        </div>
      }
      actions={
        <>
          <Button variant="ghost" size="icon" className="p-2 h-auto w-auto">
            <Play className="w-4 h-4" />
          </Button>
          <Button variant="ghost" size="icon" className="p-2 h-auto w-auto">
            <Pencil className="w-4 h-4" />
          </Button>
          <Button variant="ghost" size="icon" className="p-2 h-auto w-auto">
            <Copy className="w-4 h-4" />
          </Button>
          <Button variant="ghost" size="icon" className="p-2 h-auto w-auto">
            <Trash2 className="w-4 h-4" />
          </Button>
        </>
      }
      showDivider
      footer={
        <div className="flex items-center justify-between">
          <div className="text-xs text-muted-foreground">
            Last execution: <span className="text-green-600">Success</span>
          </div>
          <Button variant="link" size="sm" className="h-auto p-0">
            View Details <ExternalLink className="w-3 h-3 ml-1" />
          </Button>
        </div>
      }
    />
  ),
};

export const GridLayout: Story = {
  name: 'Grid Layout',
  render: () => (
    <div className="grid grid-cols-2 gap-4 max-w-4xl">
      <EntityTile
        kicker="v1"
        title="Order Sync"
        description="Synchronizes orders between platforms."
        metadata={[
          <>
            <Clock className="w-3.5 h-3.5" />1 hr ago
          </>,
        ]}
      />
      <EntityTile
        kicker="v2"
        title="Customer Import"
        description="Imports new customers from CSV."
        badges={
          <Badge variant="success" className="text-[10px]">
            Active
          </Badge>
        }
        metadata={[
          <>
            <Clock className="w-3.5 h-3.5" />3 hrs ago
          </>,
        ]}
      />
      <EntityTile
        kicker="Draft"
        title="Product Update"
        description="Updates product information."
        badges={
          <Badge variant="warning" className="text-[10px]">
            Draft
          </Badge>
        }
      />
      <EntityTile
        kicker="v1"
        title="Inventory Check"
        description="Checks inventory levels."
        metadata={[
          <>
            <Calendar className="w-3.5 h-3.5" />
            Jan 20, 2024
          </>,
        ]}
      />
    </div>
  ),
};

export const Minimal: Story = {
  args: {
    title: 'Simple Entity',
  },
};

export const LongContent: Story = {
  name: 'Long Content',
  args: {
    kicker: 'v1',
    title:
      'This is a very long title that might need to be truncated on smaller screens',
    description:
      'This is a very long description that demonstrates how the EntityTile handles longer content. It should be truncated with an ellipsis after two lines to maintain a consistent card height across the grid.',
  },
};
