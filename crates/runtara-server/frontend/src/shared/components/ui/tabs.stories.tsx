import type { Meta, StoryObj } from '@storybook/react';
import { useState } from 'react';
import { Tabs, TabsContent, TabsList, TabsTrigger } from './tabs';
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from './card';
import { Button } from './button';
import { Input } from './input';
import { Settings, User, Bell, Shield } from 'lucide-react';

const meta: Meta<typeof Tabs> = {
  title: 'UI/Tabs',
  component: Tabs,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'A tabs component built on Radix UI Tabs. Supports multiple tab panels with consistent styling.',
      },
    },
  },
  tags: ['autodocs'],
};

export default meta;
type Story = StoryObj<typeof Tabs>;

export const Default: Story = {
  render: () => (
    <Tabs defaultValue="tab1" className="w-[400px]">
      <TabsList>
        <TabsTrigger value="tab1">Tab 1</TabsTrigger>
        <TabsTrigger value="tab2">Tab 2</TabsTrigger>
        <TabsTrigger value="tab3">Tab 3</TabsTrigger>
      </TabsList>
      <TabsContent value="tab1">
        <p className="text-sm text-muted-foreground p-4">Content for Tab 1</p>
      </TabsContent>
      <TabsContent value="tab2">
        <p className="text-sm text-muted-foreground p-4">Content for Tab 2</p>
      </TabsContent>
      <TabsContent value="tab3">
        <p className="text-sm text-muted-foreground p-4">Content for Tab 3</p>
      </TabsContent>
    </Tabs>
  ),
};

export const WithCards: Story = {
  name: 'With Cards',
  render: () => (
    <Tabs defaultValue="account" className="w-[500px]">
      <TabsList className="grid w-full grid-cols-2">
        <TabsTrigger value="account">Account</TabsTrigger>
        <TabsTrigger value="password">Password</TabsTrigger>
      </TabsList>
      <TabsContent value="account">
        <Card>
          <CardHeader>
            <CardTitle>Account</CardTitle>
            <CardDescription>
              Make changes to your account here.
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="space-y-1">
              <label className="text-sm font-medium">Name</label>
              <Input defaultValue="John Doe" />
            </div>
            <div className="space-y-1">
              <label className="text-sm font-medium">Email</label>
              <Input defaultValue="john@example.com" />
            </div>
            <Button>Save Changes</Button>
          </CardContent>
        </Card>
      </TabsContent>
      <TabsContent value="password">
        <Card>
          <CardHeader>
            <CardTitle>Password</CardTitle>
            <CardDescription>Change your password here.</CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="space-y-1">
              <label className="text-sm font-medium">Current Password</label>
              <Input type="password" />
            </div>
            <div className="space-y-1">
              <label className="text-sm font-medium">New Password</label>
              <Input type="password" />
            </div>
            <Button>Update Password</Button>
          </CardContent>
        </Card>
      </TabsContent>
    </Tabs>
  ),
};

export const WithIcons: Story = {
  name: 'With Icons',
  render: () => (
    <Tabs defaultValue="profile" className="w-[500px]">
      <TabsList className="grid w-full grid-cols-4">
        <TabsTrigger value="profile" className="flex items-center gap-2">
          <User className="w-4 h-4" />
          <span className="hidden sm:inline">Profile</span>
        </TabsTrigger>
        <TabsTrigger value="notifications" className="flex items-center gap-2">
          <Bell className="w-4 h-4" />
          <span className="hidden sm:inline">Alerts</span>
        </TabsTrigger>
        <TabsTrigger value="security" className="flex items-center gap-2">
          <Shield className="w-4 h-4" />
          <span className="hidden sm:inline">Security</span>
        </TabsTrigger>
        <TabsTrigger value="settings" className="flex items-center gap-2">
          <Settings className="w-4 h-4" />
          <span className="hidden sm:inline">Settings</span>
        </TabsTrigger>
      </TabsList>
      <TabsContent value="profile" className="p-4">
        <h3 className="font-medium mb-2">Profile Settings</h3>
        <p className="text-sm text-muted-foreground">
          Manage your public profile information.
        </p>
      </TabsContent>
      <TabsContent value="notifications" className="p-4">
        <h3 className="font-medium mb-2">Notification Preferences</h3>
        <p className="text-sm text-muted-foreground">
          Configure how you receive notifications.
        </p>
      </TabsContent>
      <TabsContent value="security" className="p-4">
        <h3 className="font-medium mb-2">Security Settings</h3>
        <p className="text-sm text-muted-foreground">
          Manage your account security options.
        </p>
      </TabsContent>
      <TabsContent value="settings" className="p-4">
        <h3 className="font-medium mb-2">General Settings</h3>
        <p className="text-sm text-muted-foreground">
          Configure application preferences.
        </p>
      </TabsContent>
    </Tabs>
  ),
};

export const Disabled: Story = {
  name: 'Disabled Tab',
  render: () => (
    <Tabs defaultValue="active" className="w-[400px]">
      <TabsList>
        <TabsTrigger value="active">Active</TabsTrigger>
        <TabsTrigger value="disabled" disabled>
          Disabled
        </TabsTrigger>
        <TabsTrigger value="another">Another</TabsTrigger>
      </TabsList>
      <TabsContent value="active">
        <p className="text-sm text-muted-foreground p-4">
          This tab is active and clickable.
        </p>
      </TabsContent>
      <TabsContent value="disabled">
        <p className="text-sm text-muted-foreground p-4">
          You cannot see this content.
        </p>
      </TabsContent>
      <TabsContent value="another">
        <p className="text-sm text-muted-foreground p-4">
          Another available tab.
        </p>
      </TabsContent>
    </Tabs>
  ),
};

// Controlled example
const ControlledTabsExample = () => {
  const [activeTab, setActiveTab] = useState('overview');

  return (
    <div className="space-y-4 w-[400px]">
      <div className="flex gap-2">
        <Button
          size="sm"
          variant="outline"
          onClick={() => setActiveTab('overview')}
        >
          Go to Overview
        </Button>
        <Button
          size="sm"
          variant="outline"
          onClick={() => setActiveTab('details')}
        >
          Go to Details
        </Button>
      </div>
      <p className="text-xs text-muted-foreground">Current tab: {activeTab}</p>
      <Tabs value={activeTab} onValueChange={setActiveTab}>
        <TabsList>
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="details">Details</TabsTrigger>
          <TabsTrigger value="history">History</TabsTrigger>
        </TabsList>
        <TabsContent value="overview" className="p-4">
          <p className="text-sm text-muted-foreground">Overview content</p>
        </TabsContent>
        <TabsContent value="details" className="p-4">
          <p className="text-sm text-muted-foreground">Details content</p>
        </TabsContent>
        <TabsContent value="history" className="p-4">
          <p className="text-sm text-muted-foreground">History content</p>
        </TabsContent>
      </Tabs>
    </div>
  );
};

export const Controlled: Story = {
  render: () => <ControlledTabsExample />,
  parameters: {
    docs: {
      description: {
        story:
          'Tabs can be controlled programmatically using `value` and `onValueChange` props.',
      },
    },
  },
};

export const VerticalLayout: Story = {
  name: 'Vertical-Style Layout',
  render: () => (
    <div className="flex gap-4 w-[500px]">
      <Tabs
        defaultValue="general"
        orientation="vertical"
        className="flex gap-4"
      >
        <TabsList className="flex flex-col h-auto">
          <TabsTrigger value="general" className="w-full justify-start">
            General
          </TabsTrigger>
          <TabsTrigger value="appearance" className="w-full justify-start">
            Appearance
          </TabsTrigger>
          <TabsTrigger value="advanced" className="w-full justify-start">
            Advanced
          </TabsTrigger>
        </TabsList>
        <div className="flex-1">
          <TabsContent value="general" className="mt-0">
            <Card>
              <CardHeader>
                <CardTitle>General Settings</CardTitle>
              </CardHeader>
              <CardContent>
                <p className="text-sm text-muted-foreground">
                  Configure general options.
                </p>
              </CardContent>
            </Card>
          </TabsContent>
          <TabsContent value="appearance" className="mt-0">
            <Card>
              <CardHeader>
                <CardTitle>Appearance</CardTitle>
              </CardHeader>
              <CardContent>
                <p className="text-sm text-muted-foreground">
                  Customize the look and feel.
                </p>
              </CardContent>
            </Card>
          </TabsContent>
          <TabsContent value="advanced" className="mt-0">
            <Card>
              <CardHeader>
                <CardTitle>Advanced</CardTitle>
              </CardHeader>
              <CardContent>
                <p className="text-sm text-muted-foreground">
                  Advanced configuration options.
                </p>
              </CardContent>
            </Card>
          </TabsContent>
        </div>
      </Tabs>
    </div>
  ),
};

export const ManyTabs: Story = {
  name: 'Many Tabs',
  render: () => (
    <Tabs defaultValue="tab1" className="w-[600px]">
      <TabsList className="w-full">
        <TabsTrigger value="tab1">Overview</TabsTrigger>
        <TabsTrigger value="tab2">Analytics</TabsTrigger>
        <TabsTrigger value="tab3">Reports</TabsTrigger>
        <TabsTrigger value="tab4">Users</TabsTrigger>
        <TabsTrigger value="tab5">Settings</TabsTrigger>
      </TabsList>
      <TabsContent value="tab1" className="p-4">
        Overview content
      </TabsContent>
      <TabsContent value="tab2" className="p-4">
        Analytics content
      </TabsContent>
      <TabsContent value="tab3" className="p-4">
        Reports content
      </TabsContent>
      <TabsContent value="tab4" className="p-4">
        Users content
      </TabsContent>
      <TabsContent value="tab5" className="p-4">
        Settings content
      </TabsContent>
    </Tabs>
  ),
};
