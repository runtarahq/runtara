import type { Meta, StoryObj } from '@storybook/react';
import {
  Card,
  CardHeader,
  CardTitle,
  CardDescription,
  CardContent,
  CardFooter,
} from './card';
import { Button } from './button';
import { Badge } from './badge';
import { Settings, User, Bell, CreditCard, Mail, Lock } from 'lucide-react';

const meta: Meta<typeof Card> = {
  title: 'UI/Card',
  component: Card,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'A container component with header, content, and footer sections. Used for grouping related content and actions.',
      },
    },
  },
  tags: ['autodocs'],
};

export default meta;
type Story = StoryObj<typeof Card>;

export const Default: Story = {
  render: () => (
    <Card className="w-[350px]">
      <CardHeader>
        <CardTitle>Card Title</CardTitle>
        <CardDescription>Card description goes here</CardDescription>
      </CardHeader>
      <CardContent>
        <p className="text-sm text-muted-foreground">
          This is the main content area of the card. You can put any content
          here.
        </p>
      </CardContent>
    </Card>
  ),
};

export const WithFooter: Story = {
  name: 'With Footer',
  render: () => (
    <Card className="w-[350px]">
      <CardHeader>
        <CardTitle>Create Project</CardTitle>
        <CardDescription>Deploy your new project in one-click</CardDescription>
      </CardHeader>
      <CardContent>
        <p className="text-sm text-muted-foreground">
          Your project will be created with default settings. You can customize
          them later.
        </p>
      </CardContent>
      <CardFooter>
        <Button variant="outline">Cancel</Button>
        <Button>Create</Button>
      </CardFooter>
    </Card>
  ),
};

export const SettingsCard: Story = {
  name: 'Settings Card',
  render: () => (
    <Card className="w-[400px]">
      <CardHeader>
        <div className="flex items-center gap-3">
          <div className="p-2 rounded-lg bg-primary/10">
            <Settings className="h-5 w-5 text-primary" />
          </div>
          <div>
            <CardTitle>Account Settings</CardTitle>
            <CardDescription>Manage your account preferences</CardDescription>
          </div>
        </div>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <User className="h-4 w-4 text-muted-foreground" />
            <span className="text-sm">Profile visibility</span>
          </div>
          <Badge variant="success">Public</Badge>
        </div>
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Bell className="h-4 w-4 text-muted-foreground" />
            <span className="text-sm">Notifications</span>
          </div>
          <Badge variant="default">Enabled</Badge>
        </div>
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <Lock className="h-4 w-4 text-muted-foreground" />
            <span className="text-sm">Two-factor auth</span>
          </div>
          <Badge variant="warning">Disabled</Badge>
        </div>
      </CardContent>
      <CardFooter>
        <Button variant="outline" className="w-full">
          Edit Settings
        </Button>
      </CardFooter>
    </Card>
  ),
};

export const NotificationCard: Story = {
  name: 'Notification Card',
  render: () => (
    <Card className="w-[350px]">
      <CardHeader className="pb-3">
        <div className="flex items-center justify-between">
          <CardTitle className="text-base">Notifications</CardTitle>
          <Badge variant="destructive">3 new</Badge>
        </div>
      </CardHeader>
      <CardContent className="space-y-3">
        <div className="flex gap-3 p-2 rounded-lg hover:bg-muted/50 cursor-pointer">
          <Mail className="h-5 w-5 text-primary shrink-0 mt-0.5" />
          <div>
            <p className="text-sm font-medium">New message received</p>
            <p className="text-xs text-muted-foreground">2 minutes ago</p>
          </div>
        </div>
        <div className="flex gap-3 p-2 rounded-lg hover:bg-muted/50 cursor-pointer">
          <CreditCard className="h-5 w-5 text-green-600 shrink-0 mt-0.5" />
          <div>
            <p className="text-sm font-medium">Payment successful</p>
            <p className="text-xs text-muted-foreground">1 hour ago</p>
          </div>
        </div>
        <div className="flex gap-3 p-2 rounded-lg hover:bg-muted/50 cursor-pointer">
          <User className="h-5 w-5 text-blue-600 shrink-0 mt-0.5" />
          <div>
            <p className="text-sm font-medium">New team member added</p>
            <p className="text-xs text-muted-foreground">3 hours ago</p>
          </div>
        </div>
      </CardContent>
      <CardFooter>
        <Button variant="ghost" className="w-full text-sm">
          View all notifications
        </Button>
      </CardFooter>
    </Card>
  ),
};

export const StatsCard: Story = {
  name: 'Stats Card',
  render: () => (
    <div className="grid grid-cols-2 gap-4">
      <Card>
        <CardHeader className="pb-2">
          <CardDescription>Total Revenue</CardDescription>
          <CardTitle className="text-2xl">$45,231.89</CardTitle>
        </CardHeader>
        <CardContent>
          <p className="text-xs text-muted-foreground">
            <span className="text-green-600">+20.1%</span> from last month
          </p>
        </CardContent>
      </Card>
      <Card>
        <CardHeader className="pb-2">
          <CardDescription>Active Users</CardDescription>
          <CardTitle className="text-2xl">+2,350</CardTitle>
        </CardHeader>
        <CardContent>
          <p className="text-xs text-muted-foreground">
            <span className="text-green-600">+180.1%</span> from last month
          </p>
        </CardContent>
      </Card>
      <Card>
        <CardHeader className="pb-2">
          <CardDescription>Sales</CardDescription>
          <CardTitle className="text-2xl">+12,234</CardTitle>
        </CardHeader>
        <CardContent>
          <p className="text-xs text-muted-foreground">
            <span className="text-green-600">+19%</span> from last month
          </p>
        </CardContent>
      </Card>
      <Card>
        <CardHeader className="pb-2">
          <CardDescription>Pending Orders</CardDescription>
          <CardTitle className="text-2xl">573</CardTitle>
        </CardHeader>
        <CardContent>
          <p className="text-xs text-muted-foreground">
            <span className="text-red-600">+201</span> since last hour
          </p>
        </CardContent>
      </Card>
    </div>
  ),
};

export const MinimalCard: Story = {
  name: 'Minimal (Content Only)',
  render: () => (
    <Card className="w-[300px]">
      <CardContent className="pt-6">
        <p className="text-sm text-muted-foreground">
          A minimal card with only content, no header or footer.
        </p>
      </CardContent>
    </Card>
  ),
};

export const InteractiveCard: Story = {
  name: 'Interactive Card',
  render: () => (
    <Card className="w-[350px] cursor-pointer transition-all hover:shadow-md hover:border-primary/50">
      <CardHeader>
        <CardTitle>Clickable Card</CardTitle>
        <CardDescription>This card has hover effects</CardDescription>
      </CardHeader>
      <CardContent>
        <p className="text-sm text-muted-foreground">
          Click this card to navigate or trigger an action.
        </p>
      </CardContent>
    </Card>
  ),
};

export const CardGrid: Story = {
  name: 'Card Grid Layout',
  render: () => (
    <div className="grid grid-cols-3 gap-4 max-w-3xl">
      {[
        'Scenarios',
        'Connections',
        'Objects',
        'Triggers',
        'Analytics',
        'Settings',
      ].map((item) => (
        <Card
          key={item}
          className="cursor-pointer hover:border-primary/50 transition-colors"
        >
          <CardHeader className="pb-2">
            <CardTitle className="text-base">{item}</CardTitle>
          </CardHeader>
          <CardContent>
            <p className="text-xs text-muted-foreground">
              Manage your {item.toLowerCase()}
            </p>
          </CardContent>
        </Card>
      ))}
    </div>
  ),
};
