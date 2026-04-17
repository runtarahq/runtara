import type { Meta, StoryObj } from '@storybook/react';
import { useState } from 'react';
import { UnsavedChangesDialog } from './unsaved-changes-dialog';
import { Button } from './ui/button';

const meta: Meta<typeof UnsavedChangesDialog> = {
  title: 'Shared/UnsavedChangesDialog',
  component: UnsavedChangesDialog,
  parameters: {
    layout: 'centered',
    docs: {
      description: {
        component:
          'An alert dialog for confirming navigation away from unsaved changes. Handles Escape key and overlay clicks appropriately.',
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
    confirmLabel: {
      control: 'text',
      description: 'Label for the confirm button',
    },
    cancelLabel: {
      control: 'text',
      description: 'Label for the cancel button',
    },
  },
};

export default meta;
type Story = StoryObj<typeof UnsavedChangesDialog>;

export const Default: Story = {
  args: {
    open: true,
    onConfirm: () => console.log('Confirmed'),
    onCancel: () => console.log('Cancelled'),
  },
};

export const CustomLabels: Story = {
  name: 'Custom Labels',
  args: {
    open: true,
    title: 'Leave Page?',
    description:
      'Your changes have not been saved. If you leave now, your work will be lost.',
    confirmLabel: 'Leave Without Saving',
    cancelLabel: 'Stay on Page',
    onConfirm: () => console.log('Confirmed'),
    onCancel: () => console.log('Cancelled'),
  },
};

export const DeleteConfirmation: Story = {
  name: 'Delete Confirmation Style',
  args: {
    open: true,
    title: 'Discard Draft?',
    description:
      'This draft has not been saved. Are you sure you want to discard it?',
    confirmLabel: 'Discard Draft',
    cancelLabel: 'Keep Editing',
    onConfirm: () => console.log('Confirmed'),
    onCancel: () => console.log('Cancelled'),
  },
};

// Interactive example
const InteractiveExample = () => {
  const [open, setOpen] = useState(false);
  const [lastAction, setLastAction] = useState<string | null>(null);

  return (
    <div className="space-y-4">
      <Button onClick={() => setOpen(true)}>Open Dialog</Button>

      <UnsavedChangesDialog
        open={open}
        onConfirm={() => {
          setLastAction('Confirmed - changes discarded');
          setOpen(false);
        }}
        onCancel={() => {
          setLastAction('Cancelled - staying on page');
          setOpen(false);
        }}
      />

      {lastAction && (
        <div className="p-3 bg-muted rounded text-sm">
          <strong>Last action:</strong> {lastAction}
        </div>
      )}
    </div>
  );
};

export const Interactive: Story = {
  render: () => <InteractiveExample />,
  parameters: {
    docs: {
      description: {
        story:
          'Click the button to open the dialog. Try pressing Escape or clicking outside to see the cancel behavior.',
      },
    },
  },
};

// Form scenario example
const FormScenarioExample = () => {
  const [open, setOpen] = useState(false);
  const [hasChanges, setHasChanges] = useState(false);
  const [formValue, setFormValue] = useState('');

  const handleNavigateAway = () => {
    if (hasChanges) {
      setOpen(true);
    } else {
      alert('No changes to save, navigating away...');
    }
  };

  return (
    <div className="space-y-4 w-[300px]">
      <div>
        <label className="text-sm font-medium">Form Field</label>
        <input
          type="text"
          value={formValue}
          onChange={(e) => {
            setFormValue(e.target.value);
            setHasChanges(e.target.value !== '');
          }}
          className="w-full mt-1 px-3 py-2 border rounded-md"
          placeholder="Type something..."
        />
      </div>

      <div className="flex gap-2">
        <Button variant="outline" onClick={handleNavigateAway}>
          Navigate Away
        </Button>
        <Button
          onClick={() => {
            setHasChanges(false);
            alert('Changes saved!');
          }}
        >
          Save
        </Button>
      </div>

      {hasChanges && (
        <p className="text-xs text-amber-600">You have unsaved changes</p>
      )}

      <UnsavedChangesDialog
        open={open}
        onConfirm={() => {
          setOpen(false);
          setHasChanges(false);
          setFormValue('');
          alert('Changes discarded, navigating away...');
        }}
        onCancel={() => setOpen(false)}
      />
    </div>
  );
};

export const FormScenario: Story = {
  name: 'Form Scenario',
  render: () => <FormScenarioExample />,
  parameters: {
    docs: {
      description: {
        story:
          'Demonstrates the dialog in a typical form scenario. Type something, then click "Navigate Away" to see the dialog.',
      },
    },
  },
};
