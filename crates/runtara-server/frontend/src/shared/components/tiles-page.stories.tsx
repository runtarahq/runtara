import type { Meta, StoryObj } from '@storybook/react';
import { TilesPage, TileList } from './tiles-page';
import { Button } from './ui/button';
import { Input } from './ui/input';
import { Badge } from './ui/badge';
import { Plus, Search, Filter, MoreHorizontal } from 'lucide-react';

const meta: Meta<typeof TilesPage> = {
  title: 'Layout/TilesPage',
  component: TilesPage,
  parameters: {
    layout: 'fullscreen',
    docs: {
      description: {
        component:
          'A page layout component for displaying lists of items as tiles/cards. Includes header with title, optional kicker, action button, and toolbar for filters.',
      },
    },
  },
  tags: ['autodocs'],
};

export default meta;
type Story = StoryObj<typeof TilesPage>;

// Sample tile card component
const SampleTile = ({
  title,
  description,
  status,
}: {
  title: string;
  description: string;
  status: 'active' | 'draft' | 'paused';
}) => (
  <div className="bg-white dark:bg-slate-800 border border-slate-200 dark:border-slate-700 rounded-lg p-4 hover:shadow-md transition-shadow">
    <div className="flex items-start justify-between">
      <div className="space-y-1">
        <h3 className="font-medium text-slate-900 dark:text-slate-100">
          {title}
        </h3>
        <p className="text-sm text-slate-500 dark:text-slate-400">
          {description}
        </p>
      </div>
      <Badge
        variant={
          status === 'active'
            ? 'default'
            : status === 'draft'
              ? 'secondary'
              : 'outline'
        }
      >
        {status}
      </Badge>
    </div>
  </div>
);

export const Default: Story = {
  render: () => (
    <TilesPage title="Scenarios" action={<Button>Create Scenario</Button>}>
      <TileList>
        <SampleTile
          title="Order Sync"
          description="Sync orders from Shopify to WooCommerce"
          status="active"
        />
        <SampleTile
          title="Inventory Update"
          description="Update inventory levels daily"
          status="active"
        />
        <SampleTile
          title="Price Sync"
          description="Sync product prices across platforms"
          status="draft"
        />
      </TileList>
    </TilesPage>
  ),
};

export const WithKicker: Story = {
  name: 'With Kicker',
  render: () => (
    <TilesPage
      kicker="Automation"
      title="Scenarios"
      action={
        <Button>
          <Plus className="w-4 h-4 mr-2" />
          New Scenario
        </Button>
      }
    >
      <TileList>
        <SampleTile
          title="Order Sync"
          description="Sync orders from Shopify to WooCommerce"
          status="active"
        />
        <SampleTile
          title="Inventory Update"
          description="Update inventory levels daily"
          status="active"
        />
      </TileList>
    </TilesPage>
  ),
};

export const WithToolbar: Story = {
  name: 'With Toolbar',
  render: () => (
    <TilesPage
      kicker="Integration"
      title="Connections"
      action={<Button>Add Connection</Button>}
      toolbar={
        <div className="flex items-center gap-3">
          <div className="relative flex-1 max-w-sm">
            <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
            <Input placeholder="Search connections..." className="pl-9" />
          </div>
          <Button variant="outline" size="sm">
            <Filter className="w-4 h-4 mr-2" />
            Filter
          </Button>
        </div>
      }
    >
      <TileList>
        <SampleTile
          title="Shopify Store"
          description="Connected to mystore.myshopify.com"
          status="active"
        />
        <SampleTile
          title="WooCommerce"
          description="Connected to shop.example.com"
          status="active"
        />
        <SampleTile
          title="Amazon SP-API"
          description="Pending authorization"
          status="paused"
        />
      </TileList>
    </TilesPage>
  ),
};

export const EmptyState: Story = {
  name: 'Empty State',
  render: () => (
    <TilesPage title="Triggers" action={<Button>Create Trigger</Button>}>
      <div className="flex flex-col items-center justify-center py-16 text-center">
        <div className="w-16 h-16 bg-slate-100 dark:bg-slate-800 rounded-full flex items-center justify-center mb-4">
          <Plus className="w-8 h-8 text-slate-400" />
        </div>
        <h3 className="font-medium text-lg mb-1">No triggers yet</h3>
        <p className="text-sm text-muted-foreground mb-4 max-w-sm">
          Create your first trigger to automate workflows based on events.
        </p>
        <Button>
          <Plus className="w-4 h-4 mr-2" />
          Create Trigger
        </Button>
      </div>
    </TilesPage>
  ),
};

export const GridLayout: Story = {
  name: 'Grid Layout',
  render: () => (
    <TilesPage
      kicker="Analytics"
      title="Dashboards"
      action={<Button>New Dashboard</Button>}
    >
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-3 gap-4">
        {[
          { title: 'Sales Overview', description: 'Daily sales metrics' },
          {
            title: 'Order Analytics',
            description: 'Order trends and patterns',
          },
          {
            title: 'Inventory Status',
            description: 'Stock levels across channels',
          },
          {
            title: 'Customer Insights',
            description: 'Customer behavior analysis',
          },
          { title: 'Performance', description: 'System performance metrics' },
          { title: 'Revenue Report', description: 'Monthly revenue breakdown' },
        ].map((item) => (
          <div
            key={item.title}
            className="bg-white dark:bg-slate-800 border rounded-lg p-4 hover:shadow-md transition-shadow"
          >
            <h3 className="font-medium mb-1">{item.title}</h3>
            <p className="text-sm text-muted-foreground">{item.description}</p>
          </div>
        ))}
      </div>
    </TilesPage>
  ),
};

export const WithComplexTiles: Story = {
  name: 'Complex Tiles',
  render: () => (
    <TilesPage
      kicker="Workflows"
      title="Scenarios"
      action={
        <Button>
          <Plus className="w-4 h-4 mr-2" />
          Create
        </Button>
      }
      toolbar={
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Badge variant="secondary">All (12)</Badge>
            <Badge variant="outline">Active (8)</Badge>
            <Badge variant="outline">Draft (3)</Badge>
            <Badge variant="outline">Paused (1)</Badge>
          </div>
          <div className="relative max-w-xs">
            <Search className="absolute left-3 top-1/2 -translate-y-1/2 h-4 w-4 text-muted-foreground" />
            <Input placeholder="Search..." className="pl-9 w-[200px]" />
          </div>
        </div>
      }
    >
      <TileList>
        {[
          {
            title: 'Order Synchronization',
            description:
              'Automatically sync orders between Shopify and fulfillment center',
            status: 'active' as const,
            runs: '1,234',
            lastRun: '2 minutes ago',
          },
          {
            title: 'Inventory Restock Alert',
            description: 'Send alerts when inventory drops below threshold',
            status: 'active' as const,
            runs: '567',
            lastRun: '1 hour ago',
          },
          {
            title: 'Price Update Workflow',
            description: 'Update prices across all channels based on rules',
            status: 'draft' as const,
            runs: '0',
            lastRun: 'Never',
          },
        ].map((scenario) => (
          <div
            key={scenario.title}
            className="bg-white dark:bg-slate-800 border rounded-lg p-4 hover:shadow-md transition-shadow"
          >
            <div className="flex items-start justify-between">
              <div className="space-y-1 flex-1">
                <div className="flex items-center gap-2">
                  <h3 className="font-medium">{scenario.title}</h3>
                  <Badge
                    variant={
                      scenario.status === 'active' ? 'default' : 'secondary'
                    }
                    className="text-xs"
                  >
                    {scenario.status}
                  </Badge>
                </div>
                <p className="text-sm text-muted-foreground">
                  {scenario.description}
                </p>
                <div className="flex items-center gap-4 text-xs text-muted-foreground pt-2">
                  <span>{scenario.runs} runs</span>
                  <span>Last run: {scenario.lastRun}</span>
                </div>
              </div>
              <Button variant="ghost" size="icon">
                <MoreHorizontal className="w-4 h-4" />
              </Button>
            </div>
          </div>
        ))}
      </TileList>
    </TilesPage>
  ),
};

export const NoAction: Story = {
  name: 'Without Action Button',
  render: () => (
    <TilesPage kicker="System" title="Recent Activity">
      <TileList>
        <div className="bg-white dark:bg-slate-800 border rounded-lg p-4">
          <div className="flex items-center gap-3">
            <div className="w-2 h-2 rounded-full bg-green-500" />
            <div>
              <p className="text-sm font-medium">
                Order #1234 synced successfully
              </p>
              <p className="text-xs text-muted-foreground">2 minutes ago</p>
            </div>
          </div>
        </div>
        <div className="bg-white dark:bg-slate-800 border rounded-lg p-4">
          <div className="flex items-center gap-3">
            <div className="w-2 h-2 rounded-full bg-yellow-500" />
            <div>
              <p className="text-sm font-medium">Inventory sync in progress</p>
              <p className="text-xs text-muted-foreground">5 minutes ago</p>
            </div>
          </div>
        </div>
        <div className="bg-white dark:bg-slate-800 border rounded-lg p-4">
          <div className="flex items-center gap-3">
            <div className="w-2 h-2 rounded-full bg-red-500" />
            <div>
              <p className="text-sm font-medium">
                Connection error: Shopify API
              </p>
              <p className="text-xs text-muted-foreground">10 minutes ago</p>
            </div>
          </div>
        </div>
      </TileList>
    </TilesPage>
  ),
};

export const CustomContentClassName: Story = {
  name: 'Custom Content Styling',
  render: () => (
    <TilesPage
      title="Custom Layout"
      contentClassName="max-w-4xl mx-auto"
      action={<Button variant="outline">Settings</Button>}
    >
      <div className="bg-white dark:bg-slate-800 border rounded-lg p-6">
        <h2 className="text-lg font-semibold mb-4">Centered Content</h2>
        <p className="text-muted-foreground">
          This content area is constrained with max-w-4xl and centered using
          mx-auto via the contentClassName prop.
        </p>
      </div>
    </TilesPage>
  ),
};
