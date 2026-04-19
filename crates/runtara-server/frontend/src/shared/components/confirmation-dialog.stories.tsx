import type { Meta, StoryObj } from '@storybook/react';
import { fn } from '@storybook/test';
import { useState } from 'react';
import { ConfirmationDialog } from './confirmation-dialog';
import { Button } from './ui/button';
import { Trash2, Copy, AlertTriangle } from 'lucide-react';

const meta: Meta<typeof ConfirmationDialog> = {
  title: 'Feedback/ConfirmationDialog',
  component: ConfirmationDialog,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'A modal dialog for confirming destructive or important actions. Features customizable title, description, loading state, and optional custom content.',
      },
    },
  },
  tags: ['autodocs'],
  argTypes: {
    open: {
      control: 'boolean',
      description: 'Whether the dialog is open',
    },
    title: {
      control: 'text',
      description: 'Dialog title',
    },
    description: {
      control: 'text',
      description: 'Dialog description',
    },
    loading: {
      control: 'boolean',
      description: 'Show loading state',
    },
  },
  args: {
    onClose: fn(),
    onConfirm: fn(),
  },
};

export default meta;
type Story = StoryObj<typeof ConfirmationDialog>;

export const Default: Story = {
  args: {
    open: true,
    title: 'Are you absolutely sure?',
    description: 'This action cannot be undone.',
  },
};

export const DeleteConfirmation: Story = {
  name: 'Delete Confirmation',
  args: {
    open: true,
    title: 'Delete Item',
    description:
      'This will permanently delete this item. This action cannot be undone.',
  },
};

export const Loading: Story = {
  args: {
    open: true,
    title: 'Deleting...',
    description: 'Please wait while we process your request.',
    loading: true,
  },
};

export const WithCustomContent: Story = {
  name: 'With Custom Content',
  args: {
    open: true,
    title: 'Confirm Clone',
    description: 'You are about to clone this workflow.',
    children: (
      <div className="p-4 bg-muted rounded-lg">
        <p className="text-sm font-medium">Workflow: Order Processing Flow</p>
        <p className="text-xs text-muted-foreground mt-1">ID: scn_abc123xyz</p>
      </div>
    ),
  },
};

export const DangerousAction: Story = {
  name: 'Dangerous Action',
  args: {
    open: true,
    title: 'Delete All Data',
    description:
      'This will permanently delete all data in this workspace. This action is irreversible.',
    children: (
      <div className="flex items-center gap-2 p-3 bg-destructive/10 border border-destructive/20 rounded-lg">
        <AlertTriangle className="h-5 w-5 text-destructive" />
        <span className="text-sm text-destructive">
          This action cannot be undone!
        </span>
      </div>
    ),
  },
};

// Interactive examples
const InteractiveDeleteExample = () => {
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(false);

  const handleConfirm = () => {
    setLoading(true);
    setTimeout(() => {
      setLoading(false);
      setOpen(false);
    }, 1500);
  };

  return (
    <div className="space-y-4">
      <Button variant="destructive" onClick={() => setOpen(true)}>
        <Trash2 className="h-4 w-4 mr-2" />
        Delete Item
      </Button>
      <ConfirmationDialog
        open={open}
        title="Delete Item"
        description="This will permanently delete this item. This action cannot be undone."
        loading={loading}
        onClose={() => setOpen(false)}
        onConfirm={handleConfirm}
      />
    </div>
  );
};

export const InteractiveDelete: Story = {
  name: 'Interactive Delete',
  render: () => <InteractiveDeleteExample />,
  parameters: {
    docs: {
      description: {
        story:
          'Click the delete button to open the dialog. Confirming shows a loading state.',
      },
    },
  },
};

const InteractiveCloneExample = () => {
  const [open, setOpen] = useState(false);
  const [loading, setLoading] = useState(false);

  const handleConfirm = () => {
    setLoading(true);
    setTimeout(() => {
      setLoading(false);
      setOpen(false);
    }, 1000);
  };

  return (
    <div className="space-y-4">
      <Button variant="outline" onClick={() => setOpen(true)}>
        <Copy className="h-4 w-4 mr-2" />
        Clone Workflow
      </Button>
      <ConfirmationDialog
        open={open}
        title="Clone Workflow"
        description="This will create a copy of the workflow with all its configuration."
        loading={loading}
        onClose={() => setOpen(false)}
        onConfirm={handleConfirm}
      >
        <div className="p-4 bg-muted rounded-lg">
          <p className="text-sm font-medium">Original: Order Processing Flow</p>
          <p className="text-xs text-muted-foreground mt-1">
            New name: Order Processing Flow (Copy)
          </p>
        </div>
      </ConfirmationDialog>
    </div>
  );
};

export const InteractiveClone: Story = {
  name: 'Interactive Clone',
  render: () => <InteractiveCloneExample />,
  parameters: {
    docs: {
      description: {
        story:
          'Example of using the dialog for non-destructive confirmations like cloning.',
      },
    },
  },
};

export const AllStates: Story = {
  name: 'All States Reference',
  render: () => (
    <div className="space-y-4 text-center">
      <p className="text-sm text-muted-foreground">
        The ConfirmationDialog is a modal component. Use the interactive stories
        above to see it in action.
      </p>
      <div className="grid grid-cols-2 gap-4 p-4 bg-muted rounded-lg text-left">
        <div>
          <p className="text-xs font-medium mb-1">Props</p>
          <ul className="text-xs text-muted-foreground space-y-1">
            <li>
              <code>open</code> - Controls visibility
            </li>
            <li>
              <code>title</code> - Dialog title
            </li>
            <li>
              <code>description</code> - Description text
            </li>
            <li>
              <code>loading</code> - Shows loading state
            </li>
            <li>
              <code>children</code> - Custom content
            </li>
          </ul>
        </div>
        <div>
          <p className="text-xs font-medium mb-1">Callbacks</p>
          <ul className="text-xs text-muted-foreground space-y-1">
            <li>
              <code>onClose</code> - Called on cancel/close
            </li>
            <li>
              <code>onConfirm</code> - Called on confirm
            </li>
          </ul>
        </div>
      </div>
    </div>
  ),
};
