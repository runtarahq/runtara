import type { Meta, StoryObj } from '@storybook/react';
import {
  Settings,
  User,
  Bell,
  Shield,
  Database,
  Key,
  Mail,
  Zap,
} from 'lucide-react';
import { FormSection } from './form-section';
import { Input } from './ui/input';
import { Label } from './ui/label';
import { Checkbox } from './ui/checkbox';

const meta: Meta<typeof FormSection> = {
  title: 'Forms/FormSection',
  component: FormSection,
  parameters: {
    layout: 'padded',
    docs: {
      description: {
        component:
          'A visual section container for grouping related form fields. Includes header with optional icon, description, and "Optional" badge.',
      },
    },
  },
  tags: ['autodocs'],
  argTypes: {
    title: {
      control: 'text',
      description: 'Section title (required)',
    },
    description: {
      control: 'text',
      description: 'Optional section description',
    },
    icon: {
      control: false,
      description: 'Optional LucideIcon component',
    },
    optional: {
      control: 'boolean',
      description: 'Show "Optional" badge',
    },
  },
};

export default meta;
type Story = StoryObj<typeof FormSection>;

export const Default: Story = {
  args: {
    title: 'Basic Information',
    children: (
      <div className="space-y-4">
        <div className="space-y-2">
          <Label htmlFor="name">Name</Label>
          <Input id="name" placeholder="Enter your name" />
        </div>
        <div className="space-y-2">
          <Label htmlFor="email">Email</Label>
          <Input id="email" type="email" placeholder="Enter your email" />
        </div>
      </div>
    ),
  },
};

export const WithIcon: Story = {
  name: 'With Icon',
  args: {
    title: 'User Settings',
    icon: User,
    children: (
      <div className="space-y-4">
        <div className="space-y-2">
          <Label htmlFor="username">Username</Label>
          <Input id="username" placeholder="Enter username" />
        </div>
        <div className="space-y-2">
          <Label htmlFor="display-name">Display Name</Label>
          <Input id="display-name" placeholder="Enter display name" />
        </div>
      </div>
    ),
  },
};

export const WithDescription: Story = {
  name: 'With Description',
  args: {
    title: 'Notification Preferences',
    description: 'Configure how and when you receive notifications.',
    icon: Bell,
    children: (
      <div className="space-y-3">
        <div className="flex items-center space-x-2">
          <Checkbox id="email-notif" />
          <Label htmlFor="email-notif">Email notifications</Label>
        </div>
        <div className="flex items-center space-x-2">
          <Checkbox id="push-notif" />
          <Label htmlFor="push-notif">Push notifications</Label>
        </div>
        <div className="flex items-center space-x-2">
          <Checkbox id="sms-notif" />
          <Label htmlFor="sms-notif">SMS notifications</Label>
        </div>
      </div>
    ),
  },
};

export const Optional: Story = {
  args: {
    title: 'Advanced Settings',
    description: 'These settings are for power users.',
    icon: Settings,
    optional: true,
    children: (
      <div className="space-y-4">
        <div className="space-y-2">
          <Label htmlFor="api-key">API Key</Label>
          <Input id="api-key" placeholder="Enter API key" />
        </div>
        <div className="space-y-2">
          <Label htmlFor="webhook">Webhook URL</Label>
          <Input id="webhook" placeholder="https://..." />
        </div>
      </div>
    ),
  },
};

export const SecuritySection: Story = {
  name: 'Security Settings',
  args: {
    title: 'Security',
    description: 'Manage your account security settings.',
    icon: Shield,
    children: (
      <div className="space-y-4">
        <div className="space-y-2">
          <Label htmlFor="current-password">Current Password</Label>
          <Input id="current-password" type="password" />
        </div>
        <div className="space-y-2">
          <Label htmlFor="new-password">New Password</Label>
          <Input id="new-password" type="password" />
        </div>
        <div className="space-y-2">
          <Label htmlFor="confirm-password">Confirm Password</Label>
          <Input id="confirm-password" type="password" />
        </div>
      </div>
    ),
  },
};

export const MultipleSections: Story = {
  name: 'Multiple Sections',
  render: () => (
    <div className="space-y-6">
      <FormSection title="Connection Details" icon={Database}>
        <div className="space-y-4">
          <div className="space-y-2">
            <Label htmlFor="host">Host</Label>
            <Input id="host" placeholder="localhost" />
          </div>
          <div className="space-y-2">
            <Label htmlFor="port">Port</Label>
            <Input id="port" placeholder="5432" />
          </div>
        </div>
      </FormSection>

      <FormSection title="Authentication" icon={Key}>
        <div className="space-y-4">
          <div className="space-y-2">
            <Label htmlFor="db-user">Username</Label>
            <Input id="db-user" placeholder="Enter username" />
          </div>
          <div className="space-y-2">
            <Label htmlFor="db-password">Password</Label>
            <Input id="db-password" type="password" />
          </div>
        </div>
      </FormSection>

      <FormSection
        title="Email Notifications"
        icon={Mail}
        description="Configure email delivery settings."
        optional
      >
        <div className="space-y-4">
          <div className="space-y-2">
            <Label htmlFor="smtp-host">SMTP Host</Label>
            <Input id="smtp-host" placeholder="smtp.example.com" />
          </div>
        </div>
      </FormSection>
    </div>
  ),
};

export const AllIconVariants: Story = {
  name: 'Icon Variants',
  render: () => (
    <div className="space-y-4">
      <FormSection title="User Settings" icon={User}>
        <p className="text-sm text-muted-foreground">User icon example</p>
      </FormSection>
      <FormSection title="Notifications" icon={Bell}>
        <p className="text-sm text-muted-foreground">Bell icon example</p>
      </FormSection>
      <FormSection title="Security" icon={Shield}>
        <p className="text-sm text-muted-foreground">Shield icon example</p>
      </FormSection>
      <FormSection title="Database" icon={Database}>
        <p className="text-sm text-muted-foreground">Database icon example</p>
      </FormSection>
      <FormSection title="Performance" icon={Zap}>
        <p className="text-sm text-muted-foreground">Zap icon example</p>
      </FormSection>
    </div>
  ),
};
