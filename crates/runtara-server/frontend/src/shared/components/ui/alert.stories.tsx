import type { Meta, StoryObj } from '@storybook/react';
import { Alert, AlertTitle, AlertDescription } from './alert';
import {
  AlertCircle,
  CheckCircle2,
  Info,
  AlertTriangle,
  Terminal,
  Rocket,
  Bug,
} from 'lucide-react';

const meta: Meta<typeof Alert> = {
  title: 'UI/Alert',
  component: Alert,
  parameters: {
    layout: 'padded',
    docs: {
      description: {
        component:
          'An alert component for displaying important messages. Supports icons and multiple variants for different severity levels.',
      },
    },
  },
  tags: ['autodocs'],
  argTypes: {
    variant: {
      control: 'select',
      options: ['default', 'destructive'],
      description: 'Visual style variant',
    },
  },
};

export default meta;
type Story = StoryObj<typeof Alert>;

export const Default: Story = {
  render: () => (
    <Alert>
      <Info className="h-4 w-4" />
      <AlertTitle>Information</AlertTitle>
      <AlertDescription>
        This is an informational alert with some helpful details.
      </AlertDescription>
    </Alert>
  ),
};

export const Destructive: Story = {
  render: () => (
    <Alert variant="destructive">
      <AlertCircle className="h-4 w-4" />
      <AlertTitle>Error</AlertTitle>
      <AlertDescription>
        Something went wrong. Please try again or contact support.
      </AlertDescription>
    </Alert>
  ),
};

export const Success: Story = {
  name: 'Success (Custom)',
  render: () => (
    <Alert className="border-green-500/50 text-green-700 dark:text-green-400 [&>svg]:text-green-600">
      <CheckCircle2 className="h-4 w-4" />
      <AlertTitle>Success</AlertTitle>
      <AlertDescription>
        Your changes have been saved successfully.
      </AlertDescription>
    </Alert>
  ),
};

export const Warning: Story = {
  name: 'Warning (Custom)',
  render: () => (
    <Alert className="border-yellow-500/50 text-yellow-700 dark:text-yellow-400 [&>svg]:text-yellow-600">
      <AlertTriangle className="h-4 w-4" />
      <AlertTitle>Warning</AlertTitle>
      <AlertDescription>
        This action may have unintended consequences. Please review before
        proceeding.
      </AlertDescription>
    </Alert>
  ),
};

export const WithoutIcon: Story = {
  name: 'Without Icon',
  render: () => (
    <Alert>
      <AlertTitle>Heads up!</AlertTitle>
      <AlertDescription>
        You can add components to your app using the CLI.
      </AlertDescription>
    </Alert>
  ),
};

export const TitleOnly: Story = {
  name: 'Title Only',
  render: () => (
    <Alert>
      <Terminal className="h-4 w-4" />
      <AlertTitle>Terminal command executed successfully</AlertTitle>
    </Alert>
  ),
};

export const DescriptionOnly: Story = {
  name: 'Description Only',
  render: () => (
    <Alert>
      <Info className="h-4 w-4" />
      <AlertDescription>
        A simple alert with only a description, no title.
      </AlertDescription>
    </Alert>
  ),
};

export const LongContent: Story = {
  name: 'Long Content',
  render: () => (
    <Alert>
      <Info className="h-4 w-4" />
      <AlertTitle>Important Notice</AlertTitle>
      <AlertDescription>
        This is a longer alert message that contains multiple sentences to
        demonstrate how the alert handles longer content. The text should wrap
        naturally and maintain proper spacing with the icon. You can include as
        much detail as needed to convey your message effectively.
      </AlertDescription>
    </Alert>
  ),
};

export const AllVariants: Story = {
  name: 'All Variants',
  render: () => (
    <div className="space-y-4">
      <Alert>
        <Info className="h-4 w-4" />
        <AlertTitle>Default Alert</AlertTitle>
        <AlertDescription>
          This is a default alert for general information.
        </AlertDescription>
      </Alert>

      <Alert variant="destructive">
        <AlertCircle className="h-4 w-4" />
        <AlertTitle>Destructive Alert</AlertTitle>
        <AlertDescription>
          This is a destructive alert for errors and critical issues.
        </AlertDescription>
      </Alert>

      <Alert className="border-green-500/50 text-green-700 dark:text-green-400 [&>svg]:text-green-600">
        <CheckCircle2 className="h-4 w-4" />
        <AlertTitle>Success Alert</AlertTitle>
        <AlertDescription>
          This is a custom success alert for positive confirmations.
        </AlertDescription>
      </Alert>

      <Alert className="border-yellow-500/50 text-yellow-700 dark:text-yellow-400 [&>svg]:text-yellow-600">
        <AlertTriangle className="h-4 w-4" />
        <AlertTitle>Warning Alert</AlertTitle>
        <AlertDescription>
          This is a custom warning alert for cautionary messages.
        </AlertDescription>
      </Alert>
    </div>
  ),
};

export const UseCases: Story = {
  name: 'Common Use Cases',
  render: () => (
    <div className="space-y-4">
      <Alert>
        <Rocket className="h-4 w-4" />
        <AlertTitle>New Feature Available</AlertTitle>
        <AlertDescription>
          We've just released workflow templates. Check them out in the
          Templates section.
        </AlertDescription>
      </Alert>

      <Alert variant="destructive">
        <Bug className="h-4 w-4" />
        <AlertTitle>Connection Failed</AlertTitle>
        <AlertDescription>
          Unable to connect to the API endpoint. Please check your connection
          settings and try again.
        </AlertDescription>
      </Alert>

      <Alert className="border-yellow-500/50 text-yellow-700 dark:text-yellow-400 [&>svg]:text-yellow-600">
        <AlertTriangle className="h-4 w-4" />
        <AlertTitle>Maintenance Scheduled</AlertTitle>
        <AlertDescription>
          System maintenance is scheduled for January 30, 2024 at 2:00 AM UTC.
          Expected downtime: 30 minutes.
        </AlertDescription>
      </Alert>

      <Alert className="border-green-500/50 text-green-700 dark:text-green-400 [&>svg]:text-green-600">
        <CheckCircle2 className="h-4 w-4" />
        <AlertTitle>Deployment Complete</AlertTitle>
        <AlertDescription>
          Your scenario has been deployed successfully and is now running.
        </AlertDescription>
      </Alert>
    </div>
  ),
};
